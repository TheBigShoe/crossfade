//! Status codes, the thread's last error, and the guard every entry point runs in.

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};

pub const CF_OK: i32 = 0;
/// A pointer the call needs was null.
pub const CF_ERR_NULL_ARGUMENT: i32 = 1;
/// A string was not UTF-8.
pub const CF_ERR_INVALID_UTF8: i32 = 2;
/// JSON that does not parse, or a field or value the shape does not have.
pub const CF_ERR_INVALID_JSON: i32 = 3;
/// The model refused: the message says why, in the app's words.
pub const CF_ERR_REFUSED: i32 = 4;
/// No job with that id.
pub const CF_ERR_NOT_FOUND: i32 = 5;
/// A player handle used from a thread other than the one that made it.
pub const CF_ERR_WRONG_THREAD: i32 = 6;
/// Something the app layer reports as failed (a thread that would not start).
pub const CF_ERR_FAILED: i32 = 7;
/// A bug inside the library; the call did nothing that can be relied on.
pub const CF_ERR_PANIC: i32 = 8;

/// A refusal with its code and the words the app shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub code: i32,
    pub message: String,
}

impl Failure {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn null(what: &str) -> Self {
        Self::new(CF_ERR_NULL_ARGUMENT, format!("{what} is null"))
    }

    pub fn refused(message: impl Into<String>) -> Self {
        Self::new(CF_ERR_REFUSED, message)
    }

    pub fn json(message: impl Into<String>) -> Self {
        Self::new(CF_ERR_INVALID_JSON, message)
    }
}

thread_local! {
    static LAST: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn set_last(message: Option<String>) {
    LAST.with(|l| *l.borrow_mut() = message);
}

/// The message of this thread's last guarded call, if it failed.
pub fn last_error() -> Option<String> {
    LAST.with(|l| l.borrow().clone())
}

/// Runs `f` as an entry point: clears the thread's last error, catches a panic, and turns
/// the outcome into a status code, leaving the message behind on failure.
pub fn guard(f: impl FnOnce() -> Result<(), Failure>) -> i32 {
    set_last(None);
    let outcome = catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|panic| {
        let what = panic
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown".to_owned());
        Err(Failure::new(
            CF_ERR_PANIC,
            format!("Crossfade hit an internal error ({what})."),
        ))
    });
    match outcome {
        Ok(()) => CF_OK,
        Err(failure) => {
            set_last(Some(failure.message));
            failure.code
        }
    }
}
