//! Status codes, the thread-local error message, and the panic guard.

use std::{
    cell::RefCell,
    ffi::{CString, c_char},
    panic::{AssertUnwindSafe, catch_unwind},
};

/// Success.
pub const FRESHEN_OK: i32 = 0;
/// The operation was cancelled through its `FreshenCancel` handle.
pub const FRESHEN_ERR_CANCELLED: i32 = 1;
/// A manifest signature did not verify against the pinned public keys.
pub const FRESHEN_ERR_SIGNATURE: i32 = 2;
/// The update, policy, or configuration is invalid.
pub const FRESHEN_ERR_INVALID: i32 = 3;
/// A package contains an unsafe path.
pub const FRESHEN_ERR_UNSAFE_PATH: i32 = 4;
/// A download or extraction exceeded its configured limit.
pub const FRESHEN_ERR_SIZE_LIMIT: i32 = 5;
/// A downloaded file did not match its signed hash.
pub const FRESHEN_ERR_HASH_MISMATCH: i32 = 6;
/// Another update owns the installation lock; retry later.
pub const FRESHEN_ERR_BUSY: i32 = 7;
/// The helper process failed.
pub const FRESHEN_ERR_HELPER: i32 = 8;
/// The installation requires recovery before another update.
pub const FRESHEN_ERR_RECOVERY: i32 = 9;
/// A filesystem or transport failure (includes host transport callback failures).
pub const FRESHEN_ERR_IO: i32 = 10;
/// A manifest or index was not valid JSON.
pub const FRESHEN_ERR_JSON: i32 = 11;
/// The built-in HTTPS transport failed.
pub const FRESHEN_ERR_HTTP: i32 = 12;
/// A package archive was malformed.
pub const FRESHEN_ERR_ZIP: i32 = 13;
/// An error this ABI version does not know how to classify. Read the message.
pub const FRESHEN_ERR_UNKNOWN: i32 = 100;
/// A null pointer, invalid UTF-8, bad size, or consumed handle was passed.
pub const FRESHEN_ERR_INVALID_ARGUMENT: i32 = 101;
/// A Rust panic was caught at the boundary. The handle involved should be discarded.
pub const FRESHEN_ERR_PANIC: i32 = 102;

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

pub(crate) fn set_message(message: &str) {
    let bytes: Vec<u8> = message.bytes().filter(|byte| *byte != 0).collect();
    LAST_ERROR.with(|slot| *slot.borrow_mut() = CString::new(bytes).ok());
}

pub(crate) fn invalid(message: &str) -> i32 {
    set_message(message);
    FRESHEN_ERR_INVALID_ARGUMENT
}

pub(crate) fn code_for(error: &freshen::Error) -> i32 {
    use freshen::Error as E;
    match error {
        E::Cancelled => FRESHEN_ERR_CANCELLED,
        E::Signature => FRESHEN_ERR_SIGNATURE,
        E::Invalid(_) => FRESHEN_ERR_INVALID,
        E::UnsafePath(_) => FRESHEN_ERR_UNSAFE_PATH,
        E::SizeLimit => FRESHEN_ERR_SIZE_LIMIT,
        E::HashMismatch(_) => FRESHEN_ERR_HASH_MISMATCH,
        E::Busy => FRESHEN_ERR_BUSY,
        E::Helper(_) => FRESHEN_ERR_HELPER,
        E::Recovery(_) => FRESHEN_ERR_RECOVERY,
        E::Io(_) => FRESHEN_ERR_IO,
        E::Json(_) => FRESHEN_ERR_JSON,
        E::Http(_) => FRESHEN_ERR_HTTP,
        E::Zip(_) => FRESHEN_ERR_ZIP,
        // `freshen::Error` is non-exhaustive: keep the message, report UNKNOWN.
        _ => FRESHEN_ERR_UNKNOWN,
    }
}

/// Record the library error's message and return its status code.
pub(crate) fn fail(error: &freshen::Error) -> i32 {
    set_message(&error.to_string());
    code_for(error)
}

/// Run `body`, converting a panic into `on_panic` instead of unwinding across
/// the C boundary.
pub(crate) fn guard<T>(on_panic: T, body: impl FnOnce() -> T) -> T {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(value) => value,
        Err(_) => {
            set_message("internal panic caught at the freshen C boundary");
            on_panic
        }
    }
}

/// Status-code entry points: `Err` already carries a recorded message.
pub(crate) fn run(body: impl FnOnce() -> Result<(), i32>) -> i32 {
    guard(FRESHEN_ERR_PANIC, || match body() {
        Ok(()) => FRESHEN_OK,
        Err(code) => code,
    })
}

/// Message for the most recent failure on this thread, or an empty string.
///
/// The pointer is valid until the next failing freshen call on the same thread.
/// Copy it immediately. Never free it.
#[unsafe(no_mangle)]
pub extern "C" fn freshen_last_error_message() -> *const c_char {
    guard(c"".as_ptr(), || {
        LAST_ERROR.with(|slot| {
            slot.borrow()
                .as_ref()
                .map_or(c"".as_ptr(), |message| message.as_ptr())
        })
    })
}
