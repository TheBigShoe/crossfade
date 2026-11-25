//! The `extern "C"` functions: each reads its arguments off the caller's pointers, calls a
//! safe function and writes the result back. Every `unsafe` block of the crate is here.
#![allow(unsafe_code)]

mod analysis;
mod player;
mod queue;
mod sheet;

pub use analysis::*;
pub use player::*;
pub use queue::*;
pub use sheet::*;

use crate::error::{Failure, guard, last_error};
use crate::{about, inspect};
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
pub(crate) unsafe fn write_out<T>(out: *mut T, value: T, what: &str) -> Result<(), Failure> {
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
pub(crate) unsafe fn string_out(out: *mut *mut c_char, text: String) -> Result<(), Failure> {
    if out.is_null() {
        return Err(Failure::null("the out pointer"));
    }
    // SAFETY: checked above; the caller promises it is valid for a write.
    unsafe { out.write(owned_string(text)) };
    Ok(())
}

/// Borrows the object behind a handle.
///
/// # Safety
/// `ptr` is null or a live handle of this type that outlives the call.
pub(crate) unsafe fn handle<'a, T>(ptr: *const T, what: &str) -> Result<&'a T, Failure> {
    // SAFETY: the caller's promise: null or a live handle.
    unsafe { ptr.as_ref() }.ok_or_else(|| Failure::null(what))
}

/// Hands a new object to the caller as a handle through `out`, or drops it when `out` is
/// null.
///
/// # Safety
/// As [`write_out`].
pub(crate) unsafe fn handle_out<T>(out: *mut *mut T, value: T) -> Result<(), Failure> {
    if out.is_null() {
        return Err(Failure::null("the out pointer"));
    }
    let ptr = Box::into_raw(Box::new(value));
    // SAFETY: checked above; the caller promises it is valid for a write.
    unsafe { out.write(ptr) };
    Ok(())
}

/// Takes back and drops a handle. Null does nothing.
///
/// # Safety
/// `ptr` is null or a handle of this type made by this library and not yet destroyed.
pub(crate) unsafe fn destroy<T>(ptr: *mut T) {
    if !ptr.is_null() {
        // SAFETY: the caller's promise: it came from `Box::into_raw` here, destroyed once.
        drop(unsafe { Box::from_raw(ptr) });
    }
}

/// Reads a caller's NUL-terminated UTF-8 string.
///
/// # Safety
/// `ptr` is null or points at a NUL-terminated string that outlives the call.
pub(crate) unsafe fn str_arg<'a>(ptr: *const c_char, what: &str) -> Result<&'a str, Failure> {
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

/// What the file at `path` is, as JSON; an unreadable file is described, not refused.
///
/// # Safety
/// `path` is null or a NUL-terminated string; `out` is null or valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_inspect(path: *const c_char, out: *mut *mut c_char) -> i32 {
    guard(|| {
        // SAFETY: the caller's promise about `path`.
        let path = unsafe { str_arg(path, "the path") }?;
        if out.is_null() {
            return Err(Failure::null("the out pointer"));
        }
        let json = inspect::json(path);
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
