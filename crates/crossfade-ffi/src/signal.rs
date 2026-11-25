//! A revision counter a foreign thread can wait on: bumped by whatever changes a model, read
//! with its state, waited past with a timeout.

use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Default)]
pub(crate) struct Signal {
    revision: Mutex<u64>,
    changed: Condvar,
}

impl Signal {
    pub(crate) fn bump(&self) {
        let mut r = self.revision.lock().unwrap_or_else(|e| e.into_inner());
        *r += 1;
        drop(r);
        self.changed.notify_all();
    }

    pub(crate) fn revision(&self) -> u64 {
        *self.revision.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Returns once the revision differs from `since` or `timeout` has passed, with the
    /// revision then. A timeout of zero only reads it.
    pub(crate) fn wait(&self, since: u64, timeout: Duration) -> u64 {
        let deadline = Instant::now().checked_add(timeout);
        let mut r = self.revision.lock().unwrap_or_else(|e| e.into_inner());
        while *r == since {
            let left = match deadline {
                Some(d) => d.saturating_duration_since(Instant::now()),
                // Too far ahead to represent: as good as forever.
                None => Duration::from_secs(3600),
            };
            if left.is_zero() {
                break;
            }
            r = self
                .changed
                .wait_timeout(r, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        *r
    }
}
