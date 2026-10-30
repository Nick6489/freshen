//! Cancellation, updater, candidate and prepared-update handles.

use crate::{
    error::{fail, guard, invalid, run},
    transport::{EventBridge, FfiTransport, FreshenEventFn, FreshenTransport},
    util::{cstr, handle, store, to_c_string},
};
use freshen::{
    Cancellation, Candidate, HttpTransport, ManifestLocation, PreparedUpdate, ReleaseManifest,
    ReleaseNotes, ReleaseSource, TrustStore, Updater,
};
use std::{
    ffi::{c_char, c_void},
    mem::size_of,
    path::Path,
    ptr, slice,
    time::Duration,
};
use url::Url;

/// Cancels operations. Thread-safe: cancel from any thread.
pub struct FreshenCancel(Cancellation);
/// Holds the update policy and transport. Create once, reuse for checks.
pub struct FreshenUpdater(Updater<FfiTransport>);
/// A verified newer release that has not been downloaded yet.
pub struct FreshenCandidate(Candidate);
/// A fully downloaded, verified and staged update. Freeing it without arming
/// deletes its temporary files and leaves the installation untouched.
pub struct FreshenPrepared(pub(crate) Option<PreparedUpdate>);

const DEFAULT_HTTP_TIMEOUT_SECS: u64 = 300;
/// Cancelling a blocked read is bounded by this timeout, so an absurd value would make a
/// stalled download uncancellable in practice.
const MAX_HTTP_TIMEOUT_SECS: u64 = 24 * 60 * 60;
/// The core library accepts 1 to 32 publisher keys (a handful during a rotation). A larger
/// count is refused before the buffer is read.
const MAX_TRUSTED_KEYS: usize = 32;
/// The library accepts at most this many manifest locations.
const MAX_MANIFEST_LOCATIONS: usize = 100;

/// Updater settings. Set `struct_size` to `sizeof(FreshenUpdaterConfig)`.
#[repr(C)]
pub struct FreshenUpdaterConfig {
    pub struct_size: usize,
    /// Signed product name that releases must match.
    pub product: *const c_char,
    /// Signed channel that releases must match, such as "stable".
    pub channel: *const c_char,
    /// Semantic version of the running application.
    pub current_version: *const c_char,
    /// Distribution target, for example "x86_64-pc-windows-msvc".
    pub target: *const c_char,
    /// `trusted_key_count` pinned Ed25519 public keys, 32 bytes each, back to back.
    pub trusted_keys: *const u8,
    /// Number of keys, from 1 to 32. A larger count is refused before the buffer is read.
    pub trusted_key_count: usize,
    /// Largest package download accepted, in bytes.
    pub max_download: u64,
    /// Largest total unpacked size accepted, in bytes.
    pub max_unpacked: u64,
    /// Built-in HTTPS transport total timeout in seconds; 0 selects 300 seconds, and the
    /// maximum is 86400 (one day). Cancelling a blocked read is bounded by this timeout.
    /// Ignored when `transport` is set.
    pub http_timeout_secs: u64,
    /// Optional custom transport, copied during creation. Null selects HTTPS.
    pub transport: *const FreshenTransport,
}

/// `FreshenSource.kind`: `a` is the URL of a JSON index of manifest locations.
pub const FRESHEN_SOURCE_INDEX: i32 = 1;
/// `FreshenSource.kind`: `a` is the manifest URL, `b` its signature URL.
pub const FRESHEN_SOURCE_MANIFEST: i32 = 2;
/// `FreshenSource.kind`: `a` is the GitHub owner, `b` the repository.
pub const FRESHEN_SOURCE_GITHUB: i32 = 3;
/// `FreshenSource.kind`: use the `locations` array (at most 100).
pub const FRESHEN_SOURCE_MANIFESTS: i32 = 4;

/// One manifest and its detached signature.
#[repr(C)]
pub struct FreshenLocation {
    pub document: *const c_char,
    pub signature: *const c_char,
}

/// Where to look for releases. Set `struct_size` to `sizeof(FreshenSource)`.
#[repr(C)]
pub struct FreshenSource {
    pub struct_size: usize,
    pub kind: i32,
    pub a: *const c_char,
    pub b: *const c_char,
    pub locations: *const FreshenLocation,
    /// Number of `locations`, at most 100. A larger count fails with
    /// `FRESHEN_ERR_SIZE_LIMIT` before the array is read.
    pub location_count: usize,
}

fn check_size(actual: usize, minimum: usize, name: &str) -> Result<(), i32> {
    if actual < minimum {
        return Err(invalid(&format!("{name}.struct_size is too small")));
    }
    Ok(())
}

fn parse_url(text: &str, name: &str) -> Result<Url, i32> {
    text.parse::<Url>()
        .map_err(|error| invalid(&format!("{name} is not a valid URL: {error}")))
}

unsafe fn cancellation(cancel: *const FreshenCancel) -> Cancellation {
    unsafe { cancel.as_ref() }.map_or_else(Cancellation::default, |handle| handle.0.clone())
}

unsafe fn build_source(source: *const FreshenSource) -> Result<ReleaseSource, i32> {
    let source = unsafe { handle(source, "source") }?;
    check_size(source.struct_size, size_of::<FreshenSource>(), "source")?;
    match source.kind {
        FRESHEN_SOURCE_INDEX => Ok(ReleaseSource::Index {
            document: parse_url(unsafe { cstr(source.a, "source.a") }?, "source.a")?,
        }),
        FRESHEN_SOURCE_MANIFEST => Ok(ReleaseSource::Manifest {
            document: parse_url(unsafe { cstr(source.a, "source.a") }?, "source.a")?,
            signature: parse_url(unsafe { cstr(source.b, "source.b") }?, "source.b")?,
        }),
        FRESHEN_SOURCE_GITHUB => Ok(ReleaseSource::GitHub {
            owner: unsafe { cstr(source.a, "source.a") }?.to_owned(),
            repository: unsafe { cstr(source.b, "source.b") }?.to_owned(),
        }),
        FRESHEN_SOURCE_MANIFESTS => {
            if source.location_count > MAX_MANIFEST_LOCATIONS {
                // Checked first, so a corrupt count never makes us read that far.
                crate::error::set_message(&format!(
                    "source lists {} manifest locations; at most {MAX_MANIFEST_LOCATIONS} are supported",
                    source.location_count
                ));
                return Err(crate::error::FRESHEN_ERR_SIZE_LIMIT);
            }
            if source.location_count > 0 && source.locations.is_null() {
                return Err(invalid("source.locations must not be null"));
            }
            let entries: &[FreshenLocation] = if source.location_count == 0 {
                &[]
            } else {
                unsafe { slice::from_raw_parts(source.locations, source.location_count) }
            };
            let mut locations = Vec::with_capacity(entries.len());
            for entry in entries {
                locations.push(ManifestLocation {
                    document: parse_url(
                        unsafe { cstr(entry.document, "location.document") }?,
                        "location.document",
                    )?,
                    signature: parse_url(
                        unsafe { cstr(entry.signature, "location.signature") }?,
                        "location.signature",
                    )?,
                });
            }
            Ok(ReleaseSource::Manifests(locations))
        }
        other => Err(invalid(&format!("unknown source kind {other}"))),
    }
}

/// Create an updater. On failure `*out_updater` is set to null.
///
/// # Safety
/// `config` must point to a valid `FreshenUpdaterConfig` whose strings are
/// NUL-terminated UTF-8 and whose key buffer holds `trusted_key_count * 32`
/// bytes. `out_updater` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_updater_new(
    config: *const FreshenUpdaterConfig,
    out_updater: *mut *mut FreshenUpdater,
) -> i32 {
    run(|| {
        unsafe { store(out_updater, ptr::null_mut(), "out_updater") }?;
        let config = unsafe { handle(config, "config") }?;
        check_size(
            config.struct_size,
            size_of::<FreshenUpdaterConfig>(),
            "config",
        )?;
        let current_version = unsafe { cstr(config.current_version, "config.current_version") }?
            .parse()
            .map_err(|error| invalid(&format!("config.current_version is not semver: {error}")))?;
        if config.trusted_key_count == 0 || config.trusted_keys.is_null() {
            return Err(invalid("at least one trusted public key is required"));
        }
        if config.trusted_key_count > MAX_TRUSTED_KEYS {
            // Checked before the buffer is read, so a corrupt count cannot run off the end.
            return Err(invalid(&format!(
                "config.trusted_key_count is {}; at most {MAX_TRUSTED_KEYS} keys are supported",
                config.trusted_key_count
            )));
        }
        let key_bytes =
            unsafe { slice::from_raw_parts(config.trusted_keys, config.trusted_key_count * 32) };
        // The buffer is exactly count * 32 bytes, so there is no remainder.
        let keys: Vec<[u8; 32]> = key_bytes.as_chunks::<32>().0.to_vec();
        let trust = TrustStore::new(keys).map_err(|error| fail(&error))?;
        let transport = match unsafe { config.transport.as_ref() } {
            None => {
                if config.http_timeout_secs > MAX_HTTP_TIMEOUT_SECS {
                    return Err(invalid(&format!(
                        "config.http_timeout_secs is {}; the maximum is {MAX_HTTP_TIMEOUT_SECS}",
                        config.http_timeout_secs
                    )));
                }
                let seconds = if config.http_timeout_secs == 0 {
                    DEFAULT_HTTP_TIMEOUT_SECS
                } else {
                    config.http_timeout_secs
                };
                FfiTransport::Http(
                    HttpTransport::new(Duration::from_secs(seconds))
                        .map_err(|error| fail(&error))?,
                )
            }
            Some(custom) => {
                check_size(
                    custom.struct_size,
                    size_of::<FreshenTransport>(),
                    "transport",
                )?;
                let download = custom
                    .download
                    .ok_or_else(|| invalid("transport.download must not be null"))?;
                FfiTransport::Callback {
                    user: custom.user,
                    download,
                }
            }
        };
        let updater = Updater {
            product: unsafe { cstr(config.product, "config.product") }?.to_owned(),
            channel: unsafe { cstr(config.channel, "config.channel") }?.to_owned(),
            current_version,
            target: unsafe { cstr(config.target, "config.target") }?.to_owned(),
            trust,
            transport,
            max_download: config.max_download,
            max_unpacked: config.max_unpacked,
        };
        unsafe {
            store(
                out_updater,
                Box::into_raw(Box::new(FreshenUpdater(updater))),
                "out_updater",
            )
        }
    })
}

/// Free an updater. Null is ignored.
///
/// # Safety
/// `updater` must be null or an updater not already freed, with no operation
/// still running on it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_updater_free(updater: *mut FreshenUpdater) {
    guard((), || {
        if !updater.is_null() {
            drop(unsafe { Box::from_raw(updater) });
        }
    });
}

/// Create a cancellation handle. Free it with `freshen_cancel_free`.
#[unsafe(no_mangle)]
pub extern "C" fn freshen_cancel_new() -> *mut FreshenCancel {
    guard(ptr::null_mut(), || {
        Box::into_raw(Box::new(FreshenCancel(Cancellation::default())))
    })
}

/// Create another handle that shares the same cancellation flag.
///
/// # Safety
/// `cancel` must be a live cancellation handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_cancel_clone(cancel: *const FreshenCancel) -> *mut FreshenCancel {
    guard(ptr::null_mut(), || {
        unsafe { cancel.as_ref() }.map_or(ptr::null_mut(), |handle| {
            Box::into_raw(Box::new(FreshenCancel(handle.0.clone())))
        })
    })
}

/// Request cancellation. Safe from any thread, and idempotent. A blocked
/// network read is bounded by the request timeout.
///
/// # Safety
/// `cancel` must be a live cancellation handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_cancel_cancel(cancel: *const FreshenCancel) {
    guard((), || {
        if let Some(handle) = unsafe { cancel.as_ref() } {
            handle.0.cancel();
        }
    });
}

/// Free a cancellation handle. Null is ignored.
///
/// # Safety
/// `cancel` must be null or a handle not already freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_cancel_free(cancel: *mut FreshenCancel) {
    guard((), || {
        if !cancel.is_null() {
            drop(unsafe { Box::from_raw(cancel) });
        }
    });
}

/// Check for a newer authenticated release. Blocking: run it on a worker thread.
/// Returns `FRESHEN_OK` with `*out_candidate` set when an update exists, or with
/// `*out_candidate` null when the application is up to date. Any failure
/// (including a failed fetch of an older release) is an error, never "up to date".
///
/// # Safety
/// Pointers must be valid as documented on their types. `cancel`, `on_event`
/// and `user` may be null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_check(
    updater: *const FreshenUpdater,
    source: *const FreshenSource,
    cancel: *const FreshenCancel,
    on_event: FreshenEventFn,
    user: *mut c_void,
    out_candidate: *mut *mut FreshenCandidate,
) -> i32 {
    run(|| {
        unsafe { store(out_candidate, ptr::null_mut(), "out_candidate") }?;
        let updater = unsafe { handle(updater, "updater") }?;
        let source = unsafe { build_source(source) }?;
        let cancellation = unsafe { cancellation(cancel) };
        let bridge = EventBridge::new(on_event, user);
        let found = updater
            .0
            .check(&source, &cancellation, &mut |event| bridge.emit(&event))
            .map_err(|error| fail(&error))?;
        if let Some(candidate) = found {
            unsafe {
                store(
                    out_candidate,
                    Box::into_raw(Box::new(FreshenCandidate(candidate))),
                    "out_candidate",
                )
            }?;
        }
        Ok(())
    })
}

/// Download, verify and extract a candidate into a private directory beneath
/// `temporary_parent`. The candidate handle stays valid and is not consumed.
/// Blocking: run it on a worker thread.
///
/// # Safety
/// Pointers must be valid as documented on their types. `cancel`, `on_event`
/// and `user` may be null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_prepare(
    updater: *const FreshenUpdater,
    candidate: *const FreshenCandidate,
    temporary_parent: *const c_char,
    cancel: *const FreshenCancel,
    on_event: FreshenEventFn,
    user: *mut c_void,
    out_prepared: *mut *mut FreshenPrepared,
) -> i32 {
    run(|| {
        unsafe { store(out_prepared, ptr::null_mut(), "out_prepared") }?;
        let updater = unsafe { handle(updater, "updater") }?;
        let candidate = unsafe { handle(candidate, "candidate") }?;
        let parent = unsafe { cstr(temporary_parent, "temporary_parent") }?;
        let cancellation = unsafe { cancellation(cancel) };
        let bridge = EventBridge::new(on_event, user);
        let prepared = updater
            .0
            .prepare(
                candidate.0.clone(),
                Path::new(parent),
                &cancellation,
                &mut |event| bridge.emit(&event),
            )
            .map_err(|error| fail(&error))?;
        unsafe {
            store(
                out_prepared,
                Box::into_raw(Box::new(FreshenPrepared(Some(prepared)))),
                "out_prepared",
            )
        }
    })
}

fn version_text(release: &ReleaseManifest) -> *mut c_char {
    to_c_string(&release.version.to_string())
}

fn history_version(history: &[ReleaseNotes], index: usize) -> *mut c_char {
    history.get(index).map_or(ptr::null_mut(), |entry| {
        to_c_string(&entry.version.to_string())
    })
}

fn history_notes(history: &[ReleaseNotes], index: usize) -> *mut c_char {
    history
        .get(index)
        .map_or(ptr::null_mut(), |entry| to_c_string(&entry.notes))
}

/// Version of the selected (newest) release. Free with `freshen_string_free`.
///
/// # Safety
/// `candidate` must be a live candidate handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_candidate_version(
    candidate: *const FreshenCandidate,
) -> *mut c_char {
    guard(ptr::null_mut(), || {
        unsafe { candidate.as_ref() }.map_or(ptr::null_mut(), |c| version_text(c.0.release()))
    })
}

/// Notes of the selected release; may be empty. Free with `freshen_string_free`.
///
/// # Safety
/// `candidate` must be a live candidate handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_candidate_notes(
    candidate: *const FreshenCandidate,
) -> *mut c_char {
    guard(ptr::null_mut(), || {
        unsafe { candidate.as_ref() }.map_or(ptr::null_mut(), |c| to_c_string(&c.0.release().notes))
    })
}

/// Size in bytes of the package that `freshen_prepare` will download; 0 for a null handle.
///
/// # Safety
/// `candidate` must be a live candidate handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_candidate_download_size(
    candidate: *const FreshenCandidate,
) -> u64 {
    guard(0, || {
        unsafe { candidate.as_ref() }.map_or(0, |c| c.0.artifact().size)
    })
}

/// Number of authenticated releases in the available history, oldest first,
/// through the selected release. Completeness is not guaranteed.
///
/// # Safety
/// `candidate` must be a live candidate handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_candidate_history_count(
    candidate: *const FreshenCandidate,
) -> usize {
    guard(0, || {
        unsafe { candidate.as_ref() }.map_or(0, |c| c.0.release_history().len())
    })
}

/// Version of history entry `index`, or null if out of range. Free with `freshen_string_free`.
///
/// # Safety
/// `candidate` must be a live candidate handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_candidate_history_version(
    candidate: *const FreshenCandidate,
    index: usize,
) -> *mut c_char {
    guard(ptr::null_mut(), || {
        unsafe { candidate.as_ref() }.map_or(ptr::null_mut(), |c| {
            history_version(c.0.release_history(), index)
        })
    })
}

/// Notes of history entry `index` (may be empty), or null if out of range.
/// Free with `freshen_string_free`.
///
/// # Safety
/// `candidate` must be a live candidate handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_candidate_history_notes(
    candidate: *const FreshenCandidate,
    index: usize,
) -> *mut c_char {
    guard(ptr::null_mut(), || {
        unsafe { candidate.as_ref() }.map_or(ptr::null_mut(), |c| {
            history_notes(c.0.release_history(), index)
        })
    })
}

/// Free a candidate. Null is ignored.
///
/// # Safety
/// `candidate` must be null or a handle not already freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_candidate_free(candidate: *mut FreshenCandidate) {
    guard((), || {
        if !candidate.is_null() {
            drop(unsafe { Box::from_raw(candidate) });
        }
    });
}

/// The staged update behind a handle, or `None` for null / already consumed.
fn prepared_release<'a>(prepared: *const FreshenPrepared) -> Option<&'a PreparedUpdate> {
    unsafe { prepared.as_ref() }.and_then(|p| p.0.as_ref())
}

/// Version of the prepared release, or null if the handle was consumed by
/// `freshen_arm`. Free with `freshen_string_free`.
///
/// # Safety
/// `prepared` must be a live prepared-update handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_prepared_version(prepared: *const FreshenPrepared) -> *mut c_char {
    guard(ptr::null_mut(), || {
        prepared_release(prepared).map_or(ptr::null_mut(), |p| version_text(p.release()))
    })
}

/// Notes of the prepared release (may be empty), or null if consumed.
/// Free with `freshen_string_free`.
///
/// # Safety
/// `prepared` must be a live prepared-update handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_prepared_notes(prepared: *const FreshenPrepared) -> *mut c_char {
    guard(ptr::null_mut(), || {
        prepared_release(prepared).map_or(ptr::null_mut(), |p| to_c_string(&p.release().notes))
    })
}

/// Number of history entries retained by the prepared update; 0 if consumed.
///
/// # Safety
/// `prepared` must be a live prepared-update handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_prepared_history_count(prepared: *const FreshenPrepared) -> usize {
    guard(0, || {
        prepared_release(prepared).map_or(0, |p| p.release_history().len())
    })
}

/// Version of history entry `index`, or null. Free with `freshen_string_free`.
///
/// # Safety
/// `prepared` must be a live prepared-update handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_prepared_history_version(
    prepared: *const FreshenPrepared,
    index: usize,
) -> *mut c_char {
    guard(ptr::null_mut(), || {
        prepared_release(prepared).map_or(ptr::null_mut(), |p| {
            history_version(p.release_history(), index)
        })
    })
}

/// Notes of history entry `index`, or null. Free with `freshen_string_free`.
///
/// # Safety
/// `prepared` must be a live prepared-update handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_prepared_history_notes(
    prepared: *const FreshenPrepared,
    index: usize,
) -> *mut c_char {
    guard(ptr::null_mut(), || {
        prepared_release(prepared).map_or(ptr::null_mut(), |p| {
            history_notes(p.release_history(), index)
        })
    })
}

/// Free a prepared update, deleting its temporary files. Null is ignored.
/// Safe after `freshen_arm` consumed it.
///
/// # Safety
/// `prepared` must be null or a handle not already freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_prepared_free(prepared: *mut FreshenPrepared) {
    guard((), || {
        if !prepared.is_null() {
            drop(unsafe { Box::from_raw(prepared) });
        }
    });
}
