//! A clock tests advance by hand.

use crossfade_core::Clock;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Clones share one time, so a test keeps a handle and gives another to the
/// code under test.
#[derive(Debug, Clone, Default)]
pub struct ManualClock {
    now: Arc<Mutex<Duration>>,
}

impl ManualClock {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn advance(&self, _by: Duration) {}

    /// Panics if `to` is earlier than the current time: [`Clock`] promises
    /// monotonic time.
    pub fn set(&self, _to: Duration) {}
}

impl Clock for ManualClock {
    fn now(&self) -> Duration {
        *self.now.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    #[test]
    fn starts_at_zero_and_advances() {
        let c = ManualClock::new();
        assert_eq!(c.now(), Duration::ZERO);
        c.advance(secs(2));
        c.advance(Duration::from_millis(500));
        assert_eq!(c.now(), Duration::from_millis(2500));
    }

    #[test]
    fn set_moves_forward_and_allows_equal() {
        let c = ManualClock::new();
        c.set(secs(5));
        assert_eq!(c.now(), secs(5));
        c.set(secs(5));
        assert_eq!(c.now(), secs(5));
    }

    #[test]
    #[should_panic(expected = "cannot go backwards")]
    fn set_backwards_panics() {
        let c = ManualClock::new();
        c.set(secs(5));
        c.set(secs(4));
    }

    #[test]
    fn clones_share_time() {
        let a = ManualClock::new();
        let b = a.clone();
        a.advance(secs(3));
        assert_eq!(b.now(), secs(3));
        b.set(secs(9));
        assert_eq!(a.now(), secs(9));
    }

    #[test]
    fn is_send_sync_and_usable_as_dyn_clock() {
        fn needs<T: Send + Sync + 'static>(_: &T) {}
        let c = ManualClock::new();
        needs(&c);
        let boxed: Box<dyn Clock + Send + Sync> = Box::new(c.clone());
        let handle = std::thread::spawn(move || boxed.now());
        c.advance(secs(1));
        // The thread may read before or after the advance: both are valid.
        assert!(handle.join().unwrap() <= secs(1));
    }
}
