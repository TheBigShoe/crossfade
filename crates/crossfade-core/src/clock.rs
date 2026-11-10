use std::time::{Duration, Instant};

/// Monotonic time since an arbitrary fixed origin. A seam so tests can drive
/// time by hand instead of sleeping.
pub trait Clock {
    fn now(&self) -> Duration;
}

#[derive(Debug, Clone, Copy)]
pub struct SystemClock {
    origin: Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

#[cfg(test)]
mod tests {
    use crate::{Clock, SystemClock};
    use std::time::Duration;

    #[test]
    fn system_clock_never_goes_backwards() {
        let clock = SystemClock::new();
        let mut last = Duration::ZERO;
        for _ in 0..10_000 {
            let now = clock.now();
            assert!(now >= last);
            last = now;
        }
    }

    #[test]
    fn clock_is_object_safe() {
        let boxed: Box<dyn Clock> = Box::new(SystemClock::new());
        assert!(boxed.now() <= boxed.now());
    }
}
