#[cfg(test)]
mod tests {
    use crate::time::{Rounding, TimeBase, Timestamp};
    use crate::{Error, Rational};
    use proptest::prelude::*;
    use std::cmp::Ordering;

    const ALL: [Rounding; 4] = [
        Rounding::NearestAwayFromZero,
        Rounding::Floor,
        Rounding::Ceil,
        Rounding::TowardZero,
    ];

    fn edge_i64() -> impl Strategy<Value = i64> {
        prop_oneof![
            Just(i64::MIN),
            Just(i64::MAX),
            Just(0),
            Just(1),
            Just(-1),
            any::<i64>(),
        ]
    }

    fn base() -> impl Strategy<Value = TimeBase> {
        (1..=i64::MAX, 1..=i64::MAX).prop_map(|(n, d)| TimeBase::new(n, d).unwrap())
    }

    fn small_base() -> impl Strategy<Value = TimeBase> {
        (1..=i64::from(i32::MAX), 1..=i64::from(i32::MAX))
            .prop_map(|(n, d)| TimeBase::new(n, d).unwrap())
    }

    fn tb(n: i64, d: i64) -> TimeBase {
        TimeBase::new(n, d).unwrap()
    }

    fn ts(t: i64, n: i64, d: i64) -> Timestamp {
        Timestamp::new(t, tb(n, d))
    }

    fn rescaled(t: Timestamp, to: TimeBase, r: Rounding) -> Result<i64, Error> {
        t.rescale(to, r).map(Timestamp::ticks)
    }

    #[test]
    fn time_base_must_be_strictly_positive() {
        assert_eq!(TimeBase::new(0, 1), Err(Error::InvalidTimeBase));
        assert_eq!(TimeBase::new(-1, 48000), Err(Error::InvalidTimeBase));
        assert_eq!(TimeBase::new(1, -48000), Err(Error::InvalidTimeBase));
        assert_eq!(TimeBase::new(1, 0), Err(Error::ZeroDenominator));
        assert_eq!(
            TimeBase::new(2, 96000).unwrap().as_rational(),
            Rational::new(1, 48000).unwrap()
        );
    }

    #[test]
    fn sample_rate_conversion_example() {
        let t = ts(48000, 1, 48000);
        for r in ALL {
            assert_eq!(rescaled(t, tb(1, 44100), r), Ok(44100));
        }
    }

    #[test]
    fn negative_tick_rounds_per_mode() {
        let t = ts(-1, 1, 3);
        let to = tb(1, 2);
        assert_eq!(rescaled(t, to, Rounding::Floor), Ok(-1));
        assert_eq!(rescaled(t, to, Rounding::Ceil), Ok(0));
        assert_eq!(rescaled(t, to, Rounding::TowardZero), Ok(0));
        assert_eq!(rescaled(t, to, Rounding::NearestAwayFromZero), Ok(-1));
    }

    #[test]
    fn ties_go_away_from_zero() {
        let to = tb(1, 2);
        assert_eq!(
            rescaled(ts(1, 1, 4), to, Rounding::NearestAwayFromZero),
            Ok(1)
        );
        assert_eq!(
            rescaled(ts(-1, 1, 4), to, Rounding::NearestAwayFromZero),
            Ok(-1)
        );
        assert_eq!(rescaled(ts(1, 1, 4), to, Rounding::Floor), Ok(0));
        assert_eq!(rescaled(ts(-1, 1, 4), to, Rounding::Floor), Ok(-1));
    }

    #[test]
    fn result_beyond_i64_is_an_error() {
        assert_eq!(
            rescaled(ts(i64::MAX, 1, 1), tb(1, 2), Rounding::Floor),
            Err(Error::Overflow)
        );
        assert_eq!(
            rescaled(ts(i64::MIN, 1, 1), tb(1, 1), Rounding::Floor),
            Ok(i64::MIN)
        );
        assert_eq!(
            rescaled(ts(i64::MIN, 1, 1), tb(1, 2), Rounding::Floor),
            Err(Error::Overflow)
        );
    }

    #[test]
    fn exact_when_the_intermediate_product_exceeds_i128() {
        let m = i64::MAX;
        let t = ts(m, m - 1, m);
        let to = tb(m, m - 1);
        assert_eq!(rescaled(t, to, Rounding::Floor), Ok(m - 2));
        assert_eq!(rescaled(t, to, Rounding::Ceil), Ok(m - 1));
        assert_eq!(rescaled(t, to, Rounding::NearestAwayFromZero), Ok(m - 2));
    }

    #[test]
    fn different_bases_compare_exactly() {
        assert_eq!(ts(1, 1, 2), ts(2, 1, 4));
        assert!(ts(1, 1, 3) < ts(1, 1, 2));
        assert!(ts(-1, 1, 2) < ts(0, 1, 7));
        assert_eq!(ts(1, 1, 2).cmp(&ts(2, 1, 4)), Ordering::Equal);
    }

    proptest! {
        #[test]
        fn rescale_to_same_base_is_identity(t in edge_i64(), b in base()) {
            for r in ALL {
                prop_assert_eq!(rescaled(Timestamp::new(t, b), b, r), Ok(t));
            }
        }

        #[test]
        fn finer_integer_multiple_round_trips(
            t in edge_i64(), n in 1..=i64::from(i32::MAX), d in 1..=i64::from(i32::MAX), k in 1..=i64::from(i32::MAX),
        ) {
            let coarse = tb(n, d);
            let fine = TimeBase::from_rational(Rational::new(n, d * k).unwrap()).unwrap();
            let start = Timestamp::new(t, coarse);
            match t.checked_mul(k) {
                Some(expected) => {
                    for r in ALL {
                        let up = start.rescale(fine, r).unwrap();
                        prop_assert_eq!(up.ticks(), expected);
                        prop_assert_eq!(up.rescale(coarse, r), Ok(start));
                    }
                }
                None => {
                    // Value is exactly t*k fine ticks, which does not fit.
                    prop_assert_eq!(rescaled(start, fine, Rounding::Floor), Err(Error::Overflow));
                }
            }
        }

        #[test]
        fn rounding_modes_are_ordered(t in edge_i64(), from in base(), to in base()) {
            let start = Timestamp::new(t, from);
            let f = rescaled(start, to, Rounding::Floor);
            let c = rescaled(start, to, Rounding::Ceil);
            let z = rescaled(start, to, Rounding::TowardZero);
            let n = rescaled(start, to, Rounding::NearestAwayFromZero);
            if let (Ok(f), Ok(c), Ok(z), Ok(n)) = (f, c, z, n) {
                prop_assert!(f <= n && n <= c);
                prop_assert!(f <= z && z <= c);
                prop_assert!(c - f <= 1);
            }
        }

        #[test]
        fn ordering_agrees_with_cross_multiplication(
            ta in any::<i32>(), tb_ in any::<i32>(), a in small_base(), b in small_base(),
        ) {
            let x = Timestamp::new(i64::from(ta), a);
            let y = Timestamp::new(i64::from(tb_), b);
            let lhs = i128::from(ta) * i128::from(a.as_rational().num()) * i128::from(b.as_rational().den());
            let rhs = i128::from(tb_) * i128::from(b.as_rational().num()) * i128::from(a.as_rational().den());
            prop_assert_eq!(x.cmp(&y), lhs.cmp(&rhs));
            prop_assert_eq!(x == y, lhs == rhs);
        }

        #[test]
        fn no_input_panics(
            t in edge_i64(), u in edge_i64(),
            n in edge_i64(), d in edge_i64(), m in edge_i64(), e in edge_i64(),
        ) {
            if let (Ok(a), Ok(b)) = (TimeBase::new(n, d), TimeBase::new(m, e)) {
                let x = Timestamp::new(t, a);
                let y = Timestamp::new(u, b);
                for r in ALL {
                    let _ = x.rescale(b, r);
                }
                let _ = x.cmp(&y);
            }
        }
    }
}
