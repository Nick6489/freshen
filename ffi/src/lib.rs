//! C ABI for freshen.
//!
//! # Contract
//!
//! * **Handles** (`FreshenUpdater`, `FreshenCandidate`, `FreshenPrepared`,
//!   `FreshenHandoff`, `FreshenCancel`) are opaque pointers freed with their
//!   `_free` function; freeing null is a no-op. A handle must not be used from
//!   two threads at once, except that `freshen_cancel_cancel` may be called from
//!   any thread.
//! * **Blocking**: `freshen_check`, `freshen_prepare` and `freshen_arm` block.
//!   Run them on a worker thread, never on a UI event thread.
//! * **Status codes**: every fallible function returns `FRESHEN_OK` (0) or a
//!   `FRESHEN_ERR_*` code. `freshen_last_error_message` describes the last
//!   failure on the calling thread.
//! * **Strings** in are NUL-terminated UTF-8. Strings out are owned by the
//!   caller and released with `freshen_string_free`.
//! * **Callbacks** run on the thread performing the operation.
//! * **Panics** never unwind across the boundary; they surface as
//!   `FRESHEN_ERR_PANIC`.
//! * **No insecure switches**: there is no option to skip signature
//!   verification or allow plain HTTP.
//!
//! Struct arguments begin with a `struct_size` field so later versions can
//! append fields; set it to `sizeof` of the struct as compiled by the host.

#![allow(
    clippy::missing_safety_doc,
    reason = "the pointer contract is documented once in the crate documentation and on each function's # Safety text"
)]

mod error;
mod install;
mod transport;
mod updater;
mod util;

pub use error::*;
pub use install::*;
pub use transport::*;
pub use updater::*;
pub use util::freshen_string_free;

/// Version of this C ABI. Hosts should compare it with the version their
/// bindings were written against and refuse to run on a mismatch.
pub const FRESHEN_ABI_VERSION: u32 = 1;

/// The ABI version implemented by the loaded library.
#[unsafe(no_mangle)]
pub extern "C" fn freshen_abi_version() -> u32 {
    FRESHEN_ABI_VERSION
}

/// Version of the freshen-ffi library as a static string. Do not free.
#[unsafe(no_mangle)]
pub extern "C" fn freshen_library_version() -> *const std::ffi::c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}
