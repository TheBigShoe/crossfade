//! The convert sheet's functions.

use super::{destroy, handle, handle_out, str_arg, string_out};
use crate::error::{Failure, guard};
use crate::sheet::Sheet;
use std::ffi::c_char;

/// A convert sheet for one file.
pub type CfSheet = Sheet;

/// Opens a sheet for the file at `path` into `*out`. An unreadable file is refused with the
/// inspector's problem.
///
/// # Safety
/// `path` is null or a NUL-terminated string; `out` is null or valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_sheet_new(path: *const c_char, out: *mut *mut CfSheet) -> i32 {
    guard(|| {
        // SAFETY: the caller's promise about `path`.
        let path = unsafe { str_arg(path, "the path") }?;
        if out.is_null() {
            return Err(Failure::null("the out pointer"));
        }
        let sheet = Sheet::open(path)?;
        // SAFETY: the caller's promise about `out`.
        unsafe { handle_out(out, sheet) }
    })
}

/// Applies a JSON object of changes (or a list of them, in order), all or none.
///
/// # Safety
/// `sheet` is null or a live sheet; `changes` is null or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_sheet_apply(sheet: *const CfSheet, changes: *const c_char) -> i32 {
    guard(|| {
        // SAFETY: the caller's promise about `sheet`.
        let sheet = unsafe { handle(sheet, "the sheet") }?;
        // SAFETY: the caller's promise about `changes`.
        let changes = unsafe { str_arg(changes, "the changes") }?;
        sheet.apply(changes)
    })
}

/// The sheet's whole state as JSON.
///
/// # Safety
/// `sheet` is null or a live sheet; `out` is null or valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_sheet_state(sheet: *const CfSheet, out: *mut *mut c_char) -> i32 {
    guard(|| {
        // SAFETY: the caller's promise about `sheet`.
        let sheet = unsafe { handle(sheet, "the sheet") }?;
        // SAFETY: the caller's promise about `out`.
        unsafe { string_out(out, sheet.state_json()) }
    })
}

/// Destroys a sheet. Null does nothing.
///
/// # Safety
/// `sheet` is null or a sheet from `cf_sheet_new` not yet destroyed, in use on no other thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_sheet_destroy(sheet: *mut CfSheet) {
    // Dropping a sheet cannot panic; guard anyway so nothing unwinds into C.
    guard(|| {
        // SAFETY: the caller's promise about `sheet`.
        unsafe { destroy(sheet) };
        Ok(())
    });
}
