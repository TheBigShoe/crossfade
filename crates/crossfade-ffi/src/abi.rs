//! The `extern "C"` functions: each reads its arguments off the caller's pointers, calls a
//! safe function and writes the result back. Every `unsafe` block of the crate is here.
#![allow(unsafe_code)]

use crate::about;
use crate::error::{Failure, guard, last_error};
use std::ffi::{CStr, CString, c_char};

/// Bumped only when an existing function changes meaning or shape; new functions keep it.
const ABI_VERSION: u32 = 1;

/// The crate's version, NUL-terminated for C.
static VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");

/// A string for the caller to free with `cf_string_free`. An interior NUL, which no JSON or
/// message of ours holds, would end the C string early, so it is replaced.
pub(crate) fn owned_string(text: String) -> *mut c_char {
    let text = if text.contains('\0') {
        text.replace('\0', "\u{fffd}")
    } else {
        text
    };
    CString::new(text).map_or(std::ptr::null_mut(), CString::into_raw)
}

/// Writes `value` through the caller's out-pointer.
///
/// # Safety
/// `out` is null or valid for one write of `T`.
#[allow(dead_code)]
unsafe fn write_out<T>(out: *mut T, value: T, what: &str) -> Result<(), Failure> {
    if out.is_null() {
        return Err(Failure::null(what));
    }
    // SAFETY: not null, and the caller promises it is valid for a write.
    unsafe { out.write(value) };
    Ok(())
}

/// Hands a new string to the caller through `out`.
///
/// # Safety
/// As [`write_out`].
unsafe fn string_out(out: *mut *mut c_char, text: String) -> Result<(), Failure> {
    if out.is_null() {
        return Err(Failure::null("the out pointer"));
    }
    // SAFETY: checked above; the caller promises it is valid for a write.
    unsafe { out.write(owned_string(text)) };
    Ok(())
}

/// Reads a caller's NUL-terminated UTF-8 string.
///
/// # Safety
/// `ptr` is null or points at a NUL-terminated string that outlives the call.
#[allow(dead_code)]
unsafe fn str_arg<'a>(ptr: *const c_char, what: &str) -> Result<&'a str, Failure> {
    if ptr.is_null() {
        return Err(Failure::null(what));
    }
    // SAFETY: not null; the caller promises a NUL-terminated string alive for the call.
    let bytes = unsafe { CStr::from_ptr(ptr) };
    bytes.to_str().map_err(|_| {
        Failure::new(
            crate::error::CF_ERR_INVALID_UTF8,
            format!("{what} is not UTF-8 text"),
        )
    })
}

/// The interface's version; see `ABI_VERSION`.
#[unsafe(no_mangle)]
pub extern "C" fn cf_abi_version() -> u32 {
    ABI_VERSION
}

/// The crate's version, a static string never to be freed.
#[unsafe(no_mangle)]
pub extern "C" fn cf_version() -> *const c_char {
    VERSION.as_ptr().cast()
}

/// "About Crossfade" as JSON.
///
/// # Safety
/// `out` is null or valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_about(out: *mut *mut c_char) -> i32 {
    guard(|| {
        let json = about::json(ABI_VERSION);
        // SAFETY: the caller's promise about `out`.
        unsafe { string_out(out, json) }
    })
}

/// Frees a string the library returned. Null does nothing.
///
/// # Safety
/// `ptr` is null or a string this library returned that has not been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_string_free(ptr: *mut c_char) {
    if ptr.is_null() {
        return;
    }
    // SAFETY: the caller promises it came from `CString::into_raw` here, freed once.
    drop(unsafe { CString::from_raw(ptr) });
}

/// The message of this thread's last call, if it failed; null otherwise. Free it with
/// `cf_string_free`.
#[unsafe(no_mangle)]
pub extern "C" fn cf_last_error_message() -> *mut c_char {
    last_error().map_or(std::ptr::null_mut(), owned_string)
}
