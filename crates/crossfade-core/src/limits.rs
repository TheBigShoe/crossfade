#[cfg(test)]
mod tests {
    use crate::{Error, Limits};

    #[test]
    fn default_is_conservative_and_non_zero() {
        let l = Limits::default();
        assert!(l.max_alloc_bytes > 0 && l.max_alloc_bytes <= 1 << 30);
        assert!(l.max_nesting_depth > 0 && l.max_nesting_depth <= 256);
        assert!(l.max_packet_bytes > 0 && l.max_packet_bytes <= l.max_alloc_bytes);
    }

    #[test]
    fn check_alloc_allows_up_to_the_limit() {
        let l = Limits {
            max_alloc_bytes: 100,
            ..Limits::default()
        };
        assert_eq!(l.check_alloc(0), Ok(()));
        assert_eq!(l.check_alloc(100), Ok(()));
        assert_eq!(
            l.check_alloc(101),
            Err(Error::AllocLimitExceeded {
                requested: 101,
                max: 100
            })
        );
        assert_eq!(
            l.check_alloc(usize::MAX),
            Err(Error::AllocLimitExceeded {
                requested: usize::MAX,
                max: 100
            })
        );
    }
}
