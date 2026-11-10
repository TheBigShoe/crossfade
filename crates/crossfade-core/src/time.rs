use crate::rational::gcd;
use crate::{Error, Rational};
use std::cmp::Ordering;

/// Seconds per tick, strictly positive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TimeBase(Rational);

impl TimeBase {
    pub fn new(num: i64, den: i64) -> Result<Self, Error> {
        Self::from_rational(Rational::new(num, den)?)
    }

    pub fn from_rational(r: Rational) -> Result<Self, Error> {
        if r.num() > 0 {
            Ok(Self(r))
        } else {
            Err(Error::InvalidTimeBase)
        }
    }

    pub fn as_rational(self) -> Rational {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rounding {
    /// Ties away from zero.
    NearestAwayFromZero,
    Floor,
    Ceil,
    TowardZero,
}

/// A point in time: `ticks` of `base` seconds each. Equality and ordering
/// compare the instant, not the representation.
#[derive(Debug, Clone, Copy)]
pub struct Timestamp {
    ticks: i64,
    base: TimeBase,
}

/// `(a * b) / d` as quotient and remainder, with `a < 2^64`, `b < d < 2^126`.
/// Horner over the bits of `a` keeps every step inside u128 (no 256-bit type).
fn mul_div_small(a: u64, b: u128, d: u128) -> (u128, u128) {
    let (mut q, mut r) = (0u128, 0u128);
    for i in (0..64).rev() {
        q <<= 1;
        r <<= 1;
        if r >= d {
            r -= d;
            q += 1;
        }
        if (a >> i) & 1 == 1 {
            r += b;
            if r >= d {
                r -= d;
                q += 1;
            }
        }
    }
    (q, r)
}

impl Timestamp {
    pub fn new(ticks: i64, base: TimeBase) -> Self {
        Self { ticks, base }
    }

    pub fn ticks(self) -> i64 {
        self.ticks
    }

    pub fn base(self) -> TimeBase {
        self.base
    }

    /// Exact conversion; errors if the result does not fit `i64`.
    pub fn rescale(self, to: TimeBase, rounding: Rounding) -> Result<Self, Error> {
        let (f, t) = (self.base.0, to.0);
        // ratio = from / to; both bases are positive so these fit u128.
        let mut num = u128::try_from(f.num()).map_err(|_| Error::Overflow)?
            * u128::try_from(t.den()).map_err(|_| Error::Overflow)?;
        let mut den = u128::try_from(f.den()).map_err(|_| Error::Overflow)?
            * u128::try_from(t.num()).map_err(|_| Error::Overflow)?;
        let g = gcd(num, den);
        (num, den) = (num / g, den / g);

        let neg = self.ticks < 0;
        let mag = self.ticks.unsigned_abs();
        // mag * (q + r/den) = mag*q + mag*r/den
        let (q, r) = (num / den, num % den);
        let whole = u128::from(mag).checked_mul(q).ok_or(Error::Overflow)?;
        let (frac, rem) = mul_div_small(mag, r, den);
        let mut out = whole.checked_add(frac).ok_or(Error::Overflow)?;

        let round_up = match rounding {
            Rounding::TowardZero => false,
            Rounding::Floor => neg && rem > 0,
            Rounding::Ceil => !neg && rem > 0,
            Rounding::NearestAwayFromZero => rem * 2 >= den,
        };
        if round_up {
            out = out.checked_add(1).ok_or(Error::Overflow)?;
        }

        let signed = i128::try_from(out).map_err(|_| Error::Overflow)?;
        let signed = if neg { -signed } else { signed };
        Ok(Self::new(
            i64::try_from(signed).map_err(|_| Error::Overflow)?,
            to,
        ))
    }
}

impl Ord for Timestamp {
    // ticks*num fits i128; split into floor and remainder so the final
    // cross-multiplication stays below 2^126.
    fn cmp(&self, other: &Self) -> Ordering {
        let split = |t: Timestamp| {
            let p = i128::from(t.ticks) * i128::from(t.base.0.num());
            let d = i128::from(t.base.0.den());
            (p.div_euclid(d), p.rem_euclid(d), d)
        };
        let (qa, ra, da) = split(*self);
        let (qb, rb, db) = split(*other);
        qa.cmp(&qb).then_with(|| (ra * db).cmp(&(rb * da)))
    }
}

impl PartialOrd for Timestamp {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Timestamp {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Timestamp {}

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
