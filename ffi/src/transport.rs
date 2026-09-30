//! Progress events and host-supplied transports.
//!
//! A host transport downloads bytes however it likes (custom authentication,
//! offline storage, test fixtures) but must hand them to freshen through a
//! [`FreshenSink`]. The sink lives on the Rust side, so the size limit,
//! cancellation and progress reporting cannot be skipped by host code. HTTPS-only
//! URLs without embedded credentials are enforced before the host is called.

use crate::{
    error::{self, fail, guard, invalid, run},
    util::handle_mut,
};
use freshen::{Cancellation, Error, Event, HttpTransport, Result, Transport};
use std::{
    ffi::{CString, c_char, c_void},
    io::{self, Write},
    ptr, slice,
};
use url::Url;

/// Event kind for an event this ABI version does not know. Ignore it.
pub const FRESHEN_EVENT_UNKNOWN: i32 = 0;
/// An update check has started.
pub const FRESHEN_EVENT_CHECKING: i32 = 1;
/// Download progress: `received` bytes of `total` (or `FRESHEN_TOTAL_UNKNOWN`).
pub const FRESHEN_EVENT_DOWNLOADING: i32 = 2;
/// The downloaded package is being verified.
pub const FRESHEN_EVENT_VERIFYING: i32 = 3;
/// A file is being extracted; `path` names it.
pub const FRESHEN_EVENT_EXTRACTING: i32 = 4;
/// The update is fully prepared.
pub const FRESHEN_EVENT_READY: i32 = 5;
/// `total` value meaning the size is not known.
pub const FRESHEN_TOTAL_UNKNOWN: u64 = u64::MAX;

/// Progress callback. Called on the thread running the operation, never
/// concurrently for one operation. `path` is non-null only for
/// `FRESHEN_EVENT_EXTRACTING` and is valid only for the duration of the call.
pub type FreshenEventFn = Option<
    unsafe extern "C" fn(
        user: *mut c_void,
        kind: i32,
        received: u64,
        total: u64,
        path: *const c_char,
    ),
>;

/// Host download callback. Fetch `url` and pass every byte to `sink` with
/// `freshen_sink_write`. Return 0 on success or a nonzero status (for example
/// the code returned by a failing `freshen_sink_write`, which must be
/// propagated). `limit` is informational; the sink enforces it.
pub type FreshenDownloadFn = Option<
    unsafe extern "C" fn(
        user: *mut c_void,
        url: *const c_char,
        sink: *mut FreshenSink,
        limit: u64,
    ) -> i32,
>;

/// Custom transport. Set `struct_size` to `sizeof(FreshenTransport)`.
#[repr(C)]
pub struct FreshenTransport {
    pub struct_size: usize,
    /// Passed back to `download` unchanged. It may be used from any thread that
    /// runs an operation with this updater, so it must be thread-safe.
    pub user: *mut c_void,
    pub download: FreshenDownloadFn,
}

#[derive(Clone, Copy)]
pub(crate) struct EventBridge {
    callback: FreshenEventFn,
    user: *mut c_void,
}
impl EventBridge {
    pub(crate) fn new(callback: FreshenEventFn, user: *mut c_void) -> Self {
        Self { callback, user }
    }

    pub(crate) fn emit(&self, event: &Event) {
        let Some(callback) = self.callback else {
            return;
        };
        let mut path = None;
        let (kind, received, total) = match event {
            Event::Checking => (FRESHEN_EVENT_CHECKING, 0, FRESHEN_TOTAL_UNKNOWN),
            Event::Downloading { received, total } => (
                FRESHEN_EVENT_DOWNLOADING,
                *received,
                total.unwrap_or(FRESHEN_TOTAL_UNKNOWN),
            ),
            Event::Verifying => (FRESHEN_EVENT_VERIFYING, 0, FRESHEN_TOTAL_UNKNOWN),
            Event::Extracting { path: name } => {
                path = CString::new(name.as_str()).ok();
                (FRESHEN_EVENT_EXTRACTING, 0, FRESHEN_TOTAL_UNKNOWN)
            }
            Event::Ready => (FRESHEN_EVENT_READY, 0, FRESHEN_TOTAL_UNKNOWN),
            // `Event` is non-exhaustive; tell the host something happened.
            _ => (FRESHEN_EVENT_UNKNOWN, 0, FRESHEN_TOTAL_UNKNOWN),
        };
        let path = path.as_ref().map_or(ptr::null(), |text| text.as_ptr());
        unsafe { callback(self.user, kind, received, total, path) };
    }
}

pub(crate) enum FfiTransport {
    Http(HttpTransport),
    Callback {
        user: *mut c_void,
        download: unsafe extern "C" fn(*mut c_void, *const c_char, *mut FreshenSink, u64) -> i32,
    },
}
// SAFETY: the host promises, in the documentation of `FreshenTransport::user`,
// that its context may be used from whichever thread runs an operation.
unsafe impl Send for FfiTransport {}
unsafe impl Sync for FfiTransport {}

impl Transport for FfiTransport {
    fn download(
        &self,
        url: &Url,
        output: &mut dyn Write,
        limit: u64,
        cancel: &Cancellation,
        events: &mut dyn FnMut(Event),
    ) -> Result<u64> {
        match self {
            Self::Http(http) => http.download(url, output, limit, cancel, events),
            Self::Callback { user, download } => {
                cancel.check()?;
                if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some()
                {
                    return Err(Error::Invalid(
                        "download URLs must use HTTPS without embedded credentials".into(),
                    ));
                }
                let c_url = CString::new(url.as_str())
                    .map_err(|_| Error::Invalid("URL contains a NUL byte".into()))?;
                let mut sink = FreshenSink::new(output, events, cancel, limit);
                let code = guard(error::FRESHEN_ERR_PANIC, || unsafe {
                    download(*user, c_url.as_ptr(), &mut sink, limit)
                });
                if let Some(error) = sink.error.take() {
                    return Err(error);
                }
                match code {
                    0 => Ok(sink.written),
                    error::FRESHEN_ERR_CANCELLED => Err(Error::Cancelled),
                    error::FRESHEN_ERR_SIZE_LIMIT => Err(Error::SizeLimit),
                    other => Err(Error::Io(io::Error::other(format!(
                        "host transport callback failed with status {other}"
                    )))),
                }
            }
        }
    }
}

/// Receives the bytes of one download. Valid only during the `download`
/// callback that was handed to it; never store it.
pub struct FreshenSink {
    out: *mut dyn Write,
    events: *mut dyn FnMut(Event),
    cancel: *const Cancellation,
    limit: u64,
    written: u64,
    total: Option<u64>,
    error: Option<Error>,
}

impl FreshenSink {
    fn new<'a>(
        output: &'a mut (dyn Write + 'a),
        events: &'a mut (dyn FnMut(Event) + 'a),
        cancel: &'a Cancellation,
        limit: u64,
    ) -> Self {
        // SAFETY: only the trait-object lifetime is erased. The sink is created
        // and dropped inside `download`, and hosts must not keep it afterwards.
        let out = unsafe {
            std::mem::transmute::<&'a mut (dyn Write + 'a), &'static mut (dyn Write + 'static)>(
                output,
            )
        };
        let events = unsafe {
            std::mem::transmute::<
                &'a mut (dyn FnMut(Event) + 'a),
                &'static mut (dyn FnMut(Event) + 'static),
            >(events)
        };
        Self {
            out,
            events,
            cancel,
            limit,
            written: 0,
            total: None,
            error: None,
        }
    }

    fn record(&mut self, error: Error) -> i32 {
        let code = fail(&error);
        self.error.get_or_insert(error);
        code
    }

    fn write(&mut self, bytes: &[u8]) -> Result<()> {
        unsafe { &*self.cancel }.check()?;
        let received = self
            .written
            .checked_add(bytes.len() as u64)
            .ok_or(Error::SizeLimit)?;
        if received > self.limit {
            return Err(Error::SizeLimit);
        }
        unsafe { &mut *self.out }.write_all(bytes)?;
        self.written = received;
        let events = unsafe { &mut *self.events };
        events(Event::Downloading {
            received,
            total: self.total,
        });
        Ok(())
    }
}

/// Append downloaded bytes. Returns 0, or a nonzero status the download
/// callback must return unchanged (`FRESHEN_ERR_CANCELLED`,
/// `FRESHEN_ERR_SIZE_LIMIT`, `FRESHEN_ERR_IO`). After a nonzero status, stop
/// downloading.
///
/// # Safety
/// `sink` must be the pointer passed to the current download callback and
/// `data` must point to `len` readable bytes (it may be null when `len` is 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_sink_write(
    sink: *mut FreshenSink,
    data: *const u8,
    len: usize,
) -> i32 {
    run(|| {
        let sink = unsafe { handle_mut(sink, "sink") }?;
        if len > 0 && data.is_null() {
            return Err(invalid("data must not be null when len is nonzero"));
        }
        if len > isize::MAX as usize {
            // No allocation can be this large, so the length is corrupt; a slice this long
            // is not even representable.
            return Err(invalid("len is larger than any buffer can be"));
        }
        let bytes: &[u8] = if len == 0 {
            &[]
        } else {
            unsafe { slice::from_raw_parts(data, len) }
        };
        match sink.write(bytes) {
            Ok(()) => Ok(()),
            Err(error) => Err(sink.record(error)),
        }
    })
}

/// Report the total download size when it is known (for example from a
/// Content-Length header), so progress can show a percentage. A total above
/// the limit fails with `FRESHEN_ERR_SIZE_LIMIT` before any bytes are written.
///
/// # Safety
/// `sink` must be the pointer passed to the current download callback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_sink_set_total(sink: *mut FreshenSink, total: u64) -> i32 {
    run(|| {
        let sink = unsafe { handle_mut(sink, "sink") }?;
        if total > sink.limit {
            return Err(sink.record(Error::SizeLimit));
        }
        sink.total = Some(total);
        Ok(())
    })
}

/// Whether the operation has been cancelled. Poll this while blocked on the
/// network, then return `FRESHEN_ERR_CANCELLED` from the download callback.
///
/// # Safety
/// `sink` must be the pointer passed to the current download callback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_sink_is_cancelled(sink: *const FreshenSink) -> bool {
    guard(true, || {
        unsafe { sink.as_ref() }.is_none_or(|sink| unsafe { &*sink.cancel }.check().is_err())
    })
}
