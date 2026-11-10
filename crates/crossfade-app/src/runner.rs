#[cfg(test)]
mod tests {
    use super::Throttle;

    #[test]
    fn known_total_emits_every_two_hundredth() {
        let mut t = Throttle::new();
        assert!(!t.admit(4, Some(1000)));
        assert!(t.admit(5, Some(1000)));
        assert!(!t.admit(9, Some(1000)));
        assert!(t.admit(10, Some(1000)));
    }

    #[test]
    fn a_small_total_emits_every_advance() {
        let mut t = Throttle::new();
        assert!(t.admit(1, Some(10)));
        assert!(!t.admit(1, Some(10)));
        assert!(t.admit(2, Some(10)));
    }

    #[test]
    fn unknown_total_emits_every_65536_frames() {
        let mut t = Throttle::new();
        assert!(!t.admit(65_535, None));
        assert!(t.admit(65_536, None));
        assert!(!t.admit(65_536 + 65_535, None));
        assert!(t.admit(131_072, None));
    }

    #[test]
    fn the_last_emitted_position_is_remembered() {
        let mut t = Throttle::new();
        assert_eq!(t.last_emitted(), 0);
        t.admit(7, Some(10));
        assert_eq!(t.last_emitted(), 7);
        t.admit(7, Some(1_000_000));
        assert_eq!(t.last_emitted(), 7);
    }
}
