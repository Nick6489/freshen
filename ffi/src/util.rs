//! Pointer and string conversion helpers shared by the entry points.

use crate::error::{guard, invalid};
use std::{
    ffi::{CStr, CString, c_char},
    ptr, slice,
};

/// Borrow a required, NUL-terminated UTF-8 string.
pub(crate) unsafe fn cstr<'a>(pointer: *const c_char, name: &str) -> Result<&'a str, i32> {
    if pointer.is_null() {
        return Err(invalid(&format!("{name} must not be null")));
    }
    unsafe { CStr::from_ptr(pointer) }
        .to_str()
        .map_err(|_| invalid(&format!("{name} must be valid UTF-8")))
}

/// The most entries a caller-supplied list may claim. Real lists (owned files, arguments)
/// hold a handful; a count beyond this is a corrupt or uninitialized value, and is refused
/// before any of the array is read.
pub(crate) const MAX_LIST_ITEMS: usize = 1 << 20;

/// Copy `count` required strings out of a C array.
pub(crate) unsafe fn cstr_list(
    pointer: *const *const c_char,
    count: usize,
    name: &str,
) -> Result<Vec<String>, i32> {
    if count == 0 {
        return Ok(Vec::new());
    }
    if count > MAX_LIST_ITEMS {
        return Err(invalid(&format!(
            "{name} claims {count} entries, more than the {MAX_LIST_ITEMS} allowed"
        )));
    }
    if pointer.is_null() {
        return Err(invalid(&format!(
            "{name} must not be null when its count is nonzero"
        )));
    }
    unsafe { slice::from_raw_parts(pointer, count) }
        .iter()
        .map(|item| unsafe { cstr(*item, name) }.map(str::to_owned))
        .collect()
}

pub(crate) unsafe fn handle<'a, T>(pointer: *const T, name: &str) -> Result<&'a T, i32> {
    unsafe { pointer.as_ref() }.ok_or_else(|| invalid(&format!("{name} must not be null")))
}

pub(crate) unsafe fn handle_mut<'a, T>(pointer: *mut T, name: &str) -> Result<&'a mut T, i32> {
    unsafe { pointer.as_mut() }.ok_or_else(|| invalid(&format!("{name} must not be null")))
}

/// Write through a required out-pointer.
pub(crate) unsafe fn store<T>(out: *mut T, value: T, name: &str) -> Result<(), i32> {
    if out.is_null() {
        return Err(invalid(&format!("{name} must not be null")));
    }
    unsafe { out.write(value) };
    Ok(())
}

/// Allocate a string for the host. Free it with `freshen_string_free`.
pub(crate) fn to_c_string(text: &str) -> *mut c_char {
    let bytes: Vec<u8> = text.bytes().filter(|byte| *byte != 0).collect();
    CString::new(bytes).map_or(ptr::null_mut(), CString::into_raw)
}

/// Release a string returned by this library. Null is ignored.
///
/// # Safety
/// `text` must be null or a pointer returned by a freshen function that
/// documents it as owned by the caller, and must not be freed twice.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_string_free(text: *mut c_char) {
    guard((), || {
        if !text.is_null() {
            drop(unsafe { CString::from_raw(text) });
        }
    });
}
