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
