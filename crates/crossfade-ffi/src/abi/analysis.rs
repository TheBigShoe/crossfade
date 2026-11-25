//! The analysis functions.

use super::{destroy, handle, handle_out, str_arg, string_out, write_out};
use crate::analysis::FileAnalysis;
use crate::error::{Failure, guard};
use std::ffi::c_char;
use std::time::Duration;

/// One file's waveform and loudness analysis, running in the background.
pub type CfAnalysis = FileAnalysis;

/// Starts analysing the file at `path` on a background thread. A file that cannot be read
/// still gives a handle, whose state ends `failed`.
///
/// # Safety
/// `path` is null or a NUL-terminated string; `out` is null or valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_analysis_new(path: *const c_char, out: *mut *mut CfAnalysis) -> i32 {
    guard(|| {
        // SAFETY: the caller's promise about `path`.
        let path = unsafe { str_arg(path, "the path") }?;
        if out.is_null() {
            return Err(Failure::null("the out pointer"));
        }
        // SAFETY: the caller's promise about `out`.
        unsafe { handle_out(out, FileAnalysis::start(path)) }
    })
}

/// The analysis's state as JSON.
///
/// # Safety
/// `analysis` is null or a live analysis; `out` is null or valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_analysis_state(
    analysis: *const CfAnalysis,
    out: *mut *mut c_char,
) -> i32 {
    guard(|| {
        // SAFETY: the caller's promise about `analysis`.
        let analysis = unsafe { handle(analysis, "the analysis") }?;
        // SAFETY: the caller's promise about `out`.
        unsafe { string_out(out, analysis.state_json()) }
    })
}

/// Blocks until the revision differs from `since` or `timeout_ms` passes; writes it.
///
/// # Safety
/// `analysis` is null or a live analysis; `revision` is null or valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_analysis_wait(
    analysis: *const CfAnalysis,
    since: u64,
    timeout_ms: u32,
    revision: *mut u64,
) -> i32 {
    guard(|| {
        // SAFETY: the caller's promise about `analysis`.
        let analysis = unsafe { handle(analysis, "the analysis") }?;
        if revision.is_null() {
            return Err(Failure::null("the revision pointer"));
        }
        let now = analysis.wait(since, Duration::from_millis(u64::from(timeout_ms)));
        // SAFETY: the caller's promise about `revision`.
        unsafe { write_out(revision, now, "the revision pointer") }
    })
}

/// Writes `columns` waveform columns of `lane` into `mins` and `maxs` and the number with
/// data (a prefix; the rest are NaN) into `*written`.
///
/// # Safety
/// `analysis` is null or a live analysis; `mins` and `maxs` are null or valid for `columns`
/// writes each (either may be null when `columns` is 0); `written` is null or valid for one
/// write.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn cf_analysis_columns(
    analysis: *const CfAnalysis,
    lane: u32,
    start_frame: u64,
    frames_per_column: u64,
    mins: *mut f32,
    maxs: *mut f32,
    columns: usize,
    written: *mut usize,
) -> i32 {
    guard(|| {
        // SAFETY: the caller's promise about `analysis`.
        let analysis = unsafe { handle(analysis, "the analysis") }?;
        if written.is_null() {
            return Err(Failure::null("the written pointer"));
        }
        let (mins, maxs): (&mut [f32], &mut [f32]) = if columns == 0 {
            (&mut [], &mut [])
        } else {
            if mins.is_null() || maxs.is_null() {
                return Err(Failure::null("a column buffer"));
            }
            // SAFETY: not null, and the caller promises room for `columns` floats in each,
            // two separate buffers the call alone touches.
            unsafe {
                (
                    std::slice::from_raw_parts_mut(mins, columns),
                    std::slice::from_raw_parts_mut(maxs, columns),
                )
            }
        };
        let lane = usize::try_from(lane).unwrap_or(usize::MAX);
        let n = analysis.columns(lane, start_frame, frames_per_column, mins, maxs)?;
        // SAFETY: the caller's promise about `written`.
        unsafe { write_out(written, n, "the written pointer") }
    })
}

/// Stops the analysis; its state ends `cancelled` (or `done`, if it finished first).
///
/// # Safety
/// `analysis` is null or a live analysis.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_analysis_cancel(analysis: *const CfAnalysis) -> i32 {
    guard(|| {
        // SAFETY: the caller's promise about `analysis`.
        unsafe { handle(analysis, "the analysis") }?.cancel();
        Ok(())
    })
}

/// Cancels and frees the analysis without waiting for its thread. Null does nothing.
///
/// # Safety
/// `analysis` is null or one from `cf_analysis_new` not yet destroyed, in use on no other
/// thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_analysis_destroy(analysis: *mut CfAnalysis) {
    guard(|| {
        // SAFETY: the caller's promise about `analysis`.
        unsafe { destroy(analysis) };
        Ok(())
    });
}
