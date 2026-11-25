//! The queue's functions.

use super::{destroy, handle, handle_out, string_out, write_out};
use crate::abi::CfSheet;
use crate::error::guard;
use crate::queue::JobQueue;
use std::ffi::c_char;
use std::time::Duration;

/// The conversion queue.
pub type CfQueue = JobQueue;

/// Makes an empty queue into `*out`.
///
/// # Safety
/// `out` is null or valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_queue_new(out: *mut *mut CfQueue) -> i32 {
    guard(|| {
        if out.is_null() {
            return Err(crate::error::Failure::null("the out pointer"));
        }
        // SAFETY: the caller's promise about `out`.
        unsafe { handle_out(out, JobQueue::new()) }
    })
}

/// Queues the request `sheet` makes and writes its job id to `*id`; refused, with the
/// sheet's problems as the message, when it has any.
///
/// # Safety
/// `queue` and `sheet` are null or live handles; `id` is null or valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_queue_add(
    queue: *const CfQueue,
    sheet: *const CfSheet,
    id: *mut u64,
) -> i32 {
    guard(|| {
        // SAFETY: the caller's promise about `queue`.
        let queue = unsafe { handle(queue, "the queue") }?;
        // SAFETY: the caller's promise about `sheet`.
        let sheet = unsafe { handle(sheet, "the sheet") }?;
        if id.is_null() {
            return Err(crate::error::Failure::null("the id pointer"));
        }
        let new = queue.add(sheet)?;
        // SAFETY: the caller's promise about `id`.
        unsafe { write_out(id, new, "the id pointer") }
    })
}

/// The queue's state as JSON.
///
/// # Safety
/// `queue` is null or a live queue; `out` is null or valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_queue_state(queue: *const CfQueue, out: *mut *mut c_char) -> i32 {
    guard(|| {
        // SAFETY: the caller's promise about `queue`.
        let queue = unsafe { handle(queue, "the queue") }?;
        // SAFETY: the caller's promise about `out`.
        unsafe { string_out(out, queue.state_json()) }
    })
}

/// Blocks until the queue's revision differs from `since` or `timeout_ms` passes, then writes
/// the revision to `*revision`. Never call it on a UI thread with a timeout above zero.
///
/// # Safety
/// `queue` is null or a live queue; `revision` is null or valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_queue_wait(
    queue: *const CfQueue,
    since: u64,
    timeout_ms: u32,
    revision: *mut u64,
) -> i32 {
    guard(|| {
        // SAFETY: the caller's promise about `queue`.
        let queue = unsafe { handle(queue, "the queue") }?;
        if revision.is_null() {
            return Err(crate::error::Failure::null("the revision pointer"));
        }
        let now = queue.wait(since, Duration::from_millis(u64::from(timeout_ms)));
        // SAFETY: the caller's promise about `revision`.
        unsafe { write_out(revision, now, "the revision pointer") }
    })
}

/// Cancels a job: a waiting one at once, a running one as soon as it next checks.
///
/// # Safety
/// `queue` is null or a live queue.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_queue_cancel(queue: *const CfQueue, id: u64) -> i32 {
    guard(|| {
        // SAFETY: the caller's promise about `queue`.
        unsafe { handle(queue, "the queue") }?.cancel(id)
    })
}

/// Queues a failed or cancelled job again in its row's place, writing the new id.
///
/// # Safety
/// `queue` is null or a live queue; `new_id` is null or valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_queue_retry(queue: *const CfQueue, id: u64, new_id: *mut u64) -> i32 {
    guard(|| {
        // SAFETY: the caller's promise about `queue`.
        let queue = unsafe { handle(queue, "the queue") }?;
        if new_id.is_null() {
            return Err(crate::error::Failure::null("the id pointer"));
        }
        let again = queue.retry(id)?;
        // SAFETY: the caller's promise about `new_id`.
        unsafe { write_out(new_id, again, "the id pointer") }
    })
}

/// Removes a job that is not running.
///
/// # Safety
/// `queue` is null or a live queue.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_queue_remove(queue: *const CfQueue, id: u64) -> i32 {
    guard(|| {
        // SAFETY: the caller's promise about `queue`.
        unsafe { handle(queue, "the queue") }?.remove(id)
    })
}

/// Removes every finished job.
///
/// # Safety
/// `queue` is null or a live queue.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_queue_clear_done(queue: *const CfQueue) -> i32 {
    guard(|| {
        // SAFETY: the caller's promise about `queue`.
        unsafe { handle(queue, "the queue") }?.clear_done();
        Ok(())
    })
}

/// Cancels every job, waits for the running one to stop (it removes its partial file first),
/// and frees the queue. Null does nothing. Blocks briefly while a job is running.
///
/// # Safety
/// `queue` is null or a queue from `cf_queue_new` not yet destroyed, in use on no other
/// thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_queue_destroy(queue: *mut CfQueue) {
    guard(|| {
        // SAFETY: the caller's promise about `queue`.
        unsafe { destroy(queue) };
        Ok(())
    });
}
