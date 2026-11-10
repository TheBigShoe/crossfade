use crate::Error;
use std::cmp::Ordering;

/// Exact fraction, always normalised: gcd 1, positive denominator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rational {
    num: i64,
    den: i64,
}

pub(crate) fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

impl Rational {
    /// Rejects a zero denominator, and results that do not fit `i64`.
    pub fn new(num: i64, den: i64) -> Result<Self, Error> {
        Self::from_i128(i128::from(num), i128::from(den))
    }

    // All arithmetic runs in i128 so `i64::MIN` never overflows on the way.
    fn from_i128(num: i128, den: i128) -> Result<Self, Error> {
        if den == 0 {
            return Err(Error::ZeroDenominator);
        }
        let g = i128::try_from(gcd(num.unsigned_abs(), den.unsigned_abs()))
            .map_err(|_| Error::Overflow)?;
        let (mut n, mut d) = (num / g, den / g);
        if d < 0 {
            (n, d) = (-n, -d);
        }
        Ok(Self {
            num: i64::try_from(n).map_err(|_| Error::Overflow)?,
            den: i64::try_from(d).map_err(|_| Error::Overflow)?,
        })
    }

    pub fn num(self) -> i64 {
        self.num
    }

    pub fn den(self) -> i64 {
        self.den
    }

    pub fn checked_add(self, rhs: Self) -> Result<Self, Error> {
        Self::from_i128(
            i128::from(self.num) * i128::from(rhs.den) + i128::from(rhs.num) * i128::from(self.den),
            i128::from(self.den) * i128::from(rhs.den),
        )
    }

    pub fn checked_sub(self, rhs: Self) -> Result<Self, Error> {
        Self::from_i128(
            i128::from(self.num) * i128::from(rhs.den) - i128::from(rhs.num) * i128::from(self.den),
            i128::from(self.den) * i128::from(rhs.den),
        )
    }

    pub fn checked_mul(self, rhs: Self) -> Result<Self, Error> {
        Self::from_i128(
            i128::from(self.num) * i128::from(rhs.num),
            i128::from(self.den) * i128::from(rhs.den),
        )
    }

    pub fn checked_div(self, rhs: Self) -> Result<Self, Error> {
        if rhs.num == 0 {
            return Err(Error::DivisionByZero);
        }
        Self::from_i128(
            i128::from(self.num) * i128::from(rhs.den),
            i128::from(self.den) * i128::from(rhs.num),
        )
    }
}

impl Ord for Rational {
    fn cmp(&self, other: &Self) -> Ordering {
        (i128::from(self.num) * i128::from(other.den))
            .cmp(&(i128::from(other.num) * i128::from(self.den)))
    }
}

impl PartialOrd for Rational {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::cmp::Ordering;

    // Edge values are over-represented so MIN/MAX paths are always exercised.
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

    fn nonzero_i64() -> impl Strategy<Value = i64> {
        edge_i64().prop_filter("non-zero", |d| *d != 0)
    }

    fn rational() -> impl Strategy<Value = Rational> {
        (edge_i64(), nonzero_i64())
            .prop_filter_map("unrepresentable", |(n, d)| Rational::new(n, d).ok())
    }

    fn cross(a: Rational, b: Rational) -> Ordering {
        (i128::from(a.num()) * i128::from(b.den()))
            .cmp(&(i128::from(b.num()) * i128::from(a.den())))
    }

    #[test]
    fn new_reduces_and_fixes_sign() {
        let r = Rational::new(6, -4).unwrap();
        assert_eq!((r.num(), r.den()), (-3, 2));
    }

    #[test]
    fn zero_denominator_is_rejected() {
        assert_eq!(Rational::new(1, 0), Err(Error::ZeroDenominator));
    }

    #[test]
    fn min_over_minus_one_is_an_error_not_a_panic() {
        assert_eq!(Rational::new(i64::MIN, -1), Err(Error::Overflow));
    }

    #[test]
    fn min_over_min_is_one() {
        assert_eq!(
            Rational::new(i64::MIN, i64::MIN),
            Ok(Rational::new(1, 1).unwrap())
        );
    }

    #[test]
    fn arithmetic_examples() {
        let half = Rational::new(1, 2).unwrap();
        let third = Rational::new(1, 3).unwrap();
        assert_eq!(half.checked_add(third), Ok(Rational::new(5, 6).unwrap()));
        assert_eq!(half.checked_sub(third), Ok(Rational::new(1, 6).unwrap()));
        assert_eq!(half.checked_mul(third), Ok(Rational::new(1, 6).unwrap()));
        assert_eq!(half.checked_div(third), Ok(Rational::new(3, 2).unwrap()));
    }

    #[test]
    fn division_by_zero_and_overflow_are_errors() {
        let zero = Rational::new(0, 1).unwrap();
        let one = Rational::new(1, 1).unwrap();
        assert_eq!(one.checked_div(zero), Err(Error::DivisionByZero));
        let max = Rational::new(i64::MAX, 1).unwrap();
        assert_eq!(max.checked_add(max), Err(Error::Overflow));
        assert_eq!(max.checked_mul(max), Err(Error::Overflow));
    }

    #[test]
    fn ordering_examples() {
        let a = Rational::new(1, 3).unwrap();
        let b = Rational::new(1, 2).unwrap();
        assert!(a < b);
        assert!(Rational::new(-1, 2).unwrap() < a);
    }

    proptest! {
        #[test]
        fn normalisation_is_idempotent(r in rational()) {
            prop_assert_eq!(Rational::new(r.num(), r.den()), Ok(r));
            prop_assert!(r.den() > 0);
        }

        #[test]
        fn equal_values_compare_equal_whatever_the_input(
            n in any::<i32>(), d in any::<i32>().prop_filter("non-zero", |d| *d != 0),
            k in any::<i32>().prop_filter("non-zero", |k| *k != 0),
        ) {
            let base = Rational::new(i64::from(n), i64::from(d)).unwrap();
            let scaled = Rational::new(i64::from(n) * i64::from(k), i64::from(d) * i64::from(k)).unwrap();
            prop_assert_eq!(base, scaled);
            prop_assert_eq!(base.cmp(&scaled), Ordering::Equal);
        }

        #[test]
        fn ordering_agrees_with_cross_multiplication(a in rational(), b in rational()) {
            prop_assert_eq!(a.cmp(&b), cross(a, b));
            prop_assert_eq!(a.partial_cmp(&b), Some(cross(a, b)));
        }

        #[test]
        fn add_and_mul_commute(a in rational(), b in rational()) {
            prop_assert_eq!(a.checked_add(b), b.checked_add(a));
            prop_assert_eq!(a.checked_mul(b), b.checked_mul(a));
        }

        #[test]
        fn add_then_sub_restores_the_original(x in rational(), y in rational()) {
            if let Ok(sum) = x.checked_add(y)
                && let Ok(back) = sum.checked_sub(y)
            {
                prop_assert_eq!(back, x);
            }
        }

        #[test]
        fn mul_then_div_restores_the_original(x in rational(), y in rational()) {
            if let Ok(p) = x.checked_mul(y)
                && let Ok(back) = p.checked_div(y)
            {
                prop_assert_eq!(back, x);
            }
        }

        #[test]
        fn no_input_panics(n in edge_i64(), d in edge_i64(), m in edge_i64(), e in edge_i64()) {
            if let (Ok(a), Ok(b)) = (Rational::new(n, d), Rational::new(m, e)) {
                let _ = (a.checked_add(b), a.checked_sub(b), a.checked_mul(b), a.checked_div(b));
                let _ = a.cmp(&b);
            }
        }
    }
}
