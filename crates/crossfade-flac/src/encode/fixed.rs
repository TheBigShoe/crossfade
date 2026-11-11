//! Fixed polynomial predictors of order 0 to 4 (RFC 9639 section 9.2.8).

use super::analyze::Sample;

pub(super) fn fold(e: i32) -> u32 {
    ((e << 1) ^ (e >> 31)) as u32
}

/// Largest depth whose order-4 residual (at most 16 times the sample) always fits an `i32`,
/// so wrapping 32-bit arithmetic is exact and needs no range check.
const NARROW_BITS: u32 = 27;

/// Fills `out` with the folded residuals of the order-`order` predictor over `x`
/// (`x.len() - order` of them). Returns false when a residual does not fit the format's
/// 32-bit limit (it must not be `i32::MIN` either); `out` is then not meaningful.
// Kept out of its callers: inlined into them under fat LTO it compiled slower.
#[inline(never)]
pub(super) fn residual<T: Sample>(x: &[T], order: usize, bps: u32, out: &mut Vec<u32>) -> bool {
    let n = x.len();
    out.clear();
    out.resize(n - order, 0);
    if bps <= NARROW_BITS {
        // Zipped shifted slices keep the loops free of bounds checks, so they vectorise.
        let o = out.iter_mut();
        match order {
            0 => o.zip(x).for_each(|(o, &v)| *o = fold(v.low32())),
            1 => o.zip(&x[1..]).zip(x).for_each(|((o, &a), &b)| {
                *o = fold(a.low32().wrapping_sub(b.low32()));
            }),
            2 => o
                .zip(&x[2..])
                .zip(&x[1..])
                .zip(x)
                .for_each(|(((o, &a), &b), &c)| {
                    let p = b.low32().wrapping_mul(2).wrapping_sub(c.low32());
                    *o = fold(a.low32().wrapping_sub(p));
                }),
            3 => o.zip(&x[3..]).zip(&x[2..]).zip(&x[1..]).zip(x).for_each(
                |((((o, &a), &b), &c), &d)| {
                    let p = b
                        .low32()
                        .wrapping_mul(3)
                        .wrapping_sub(c.low32().wrapping_mul(3))
                        .wrapping_add(d.low32());
                    *o = fold(a.low32().wrapping_sub(p));
                },
            ),
            _ => o
                .zip(&x[4..])
                .zip(&x[3..])
                .zip(&x[2..])
                .zip(&x[1..])
                .zip(x)
                .for_each(|(((((o, &a), &b), &c), &d), &e)| {
                    let p = b
                        .low32()
                        .wrapping_mul(4)
                        .wrapping_sub(c.low32().wrapping_mul(6))
                        .wrapping_add(d.low32().wrapping_mul(4))
                        .wrapping_sub(e.low32());
                    *o = fold(a.low32().wrapping_sub(p));
                }),
        }
        return true;
    }
    let g = |i: usize| -> i64 { x[i].into() };
    let mut bad = false;
    for (i, o) in out.iter_mut().enumerate() {
        let p = match order {
            0 => 0,
            1 => g(i),
            2 => 2 * g(i + 1) - g(i),
            3 => 3 * g(i + 2) - 3 * g(i + 1) + g(i),
            _ => 4 * g(i + 3) - 6 * g(i + 2) + 4 * g(i + 1) - g(i),
        };
        let e = g(i + order) - p;
        bad |= e <= i64::from(i32::MIN) || e > i64::from(i32::MAX);
        *o = fold(e as i32);
    }
    !bad
}

/// log2 in 1/256 bits, piecewise linear between powers of two (error under 0.09 bit).
fn log2_q8(x: u64) -> u64 {
    if x == 0 {
        return 0;
    }
    let b = 63 - x.leading_zeros();
    let frac = ((x << (63 - b)) >> 55) & 0xFF;
    u64::from(b) * 256 + frac
}

/// Sums of absolute values of the order 0 to 4 residuals over `x[4..]`, the same samples for
/// every order so the sums compare; for n < 5 only order 0 is meaningful.
pub(super) fn abs_sums<T: Sample>(x: &[T]) -> [u64; 5] {
    if let Some(x) = T::as_i32(x) {
        let peak = x.iter().fold(0u32, |m, &v| m.max(v.unsigned_abs()));
        // Below 2^26 every difference up to order 4 (at most 16 times the peak) fits an
        // i32 and a run of them a u32 sum.
        if peak < 1 << 26 && x.len() >= 5 {
            return abs_sums_narrow(x, peak);
        }
    }
    abs_sums_wide(x)
}

/// [`abs_sums`] in 32-bit lanes: sums of `2^31 / (16 peak)` differences at a time cannot
/// overflow, and then go into the 64-bit totals.
#[inline(never)]
fn abs_sums_narrow(x: &[i32], peak: u32) -> [u64; 5] {
    let biggest = 16 * u64::from(peak.max(1));
    let run = ((1u64 << 31) / biggest).clamp(1, 4096) as usize;
    let mut sums = [0u64; 5];
    let (a, b, c, d, e) = (&x[4..], &x[3..], &x[2..], &x[1..], x);
    let mut at = 0;
    while at < a.len() {
        let end = (at + run).min(a.len());
        let mut part = [0u32; 5];
        for i in at..end {
            let (a, b, c, d, e) = (a[i], b[i], c[i], d[i], e[i]);
            let d1 = a - b;
            let d2 = d1 - (b - c);
            let d3 = d2 - (b - c - (c - d));
            let d4 = d3 - (b - c - (c - d) - (c - d - (d - e)));
            for (p, v) in part.iter_mut().zip([a, d1, d2, d3, d4]) {
                *p += v.unsigned_abs();
            }
        }
        for (s, p) in sums.iter_mut().zip(part) {
            *s += u64::from(p);
        }
        at = end;
    }
    sums
}

fn abs_sums_wide<T: Sample>(x: &[T]) -> [u64; 5] {
    let n = x.len();
    let mut sums = [0u64; 5];
    let g = |i: usize| -> i64 { x[i].into() };
    if n < 5 {
        sums[0] = (0..n).map(|i| g(i).unsigned_abs()).sum();
        return sums;
    }
    for i in 4..n {
        let (a, b, c, d, e) = (g(i), g(i - 1), g(i - 2), g(i - 3), g(i - 4));
        let d1 = a - b;
        let d2 = d1 - (b - c);
        let d3 = d2 - (b - c - (c - d));
        let d4 = d3 - (b - c - (c - d) - (c - d - (d - e)));
        for (sum, v) in sums.iter_mut().zip([a, d1, d2, d3, d4]) {
            *sum += v.unsigned_abs();
        }
    }
    sums
}

/// A rough size in 1/256 bits of coding `count` values whose absolute values sum to `sum`.
pub(super) fn rough_bits(sum: u64, count: u64) -> u64 {
    count * (log2_q8(sum + count) - log2_q8(count))
}

/// A rough size in 1/256 bits of the best fixed-order coding of `x`, from the mean absolute
/// residual of each order. Only for ranking: stereo modes before a full search, and which
/// fixed orders deserve one.
pub(super) fn estimate<T: Sample>(x: &[T]) -> u64 {
    let count = x.len() as u64;
    abs_sums(x)
        .iter()
        .map(|&s| rough_bits(s, count))
        .min()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    proptest::proptest! {
        /// The 32-bit-lane sums equal the 64-bit ones, up to the largest peak they take.
        #[test]
        fn narrow_abs_sums_equal_the_wide_ones(
            seed in proptest::prelude::any::<u64>(),
            bits in 1u32..=27,
            n in 5usize..5000,
            extreme in proptest::prelude::any::<bool>(),
        ) {
            let mut s = seed | 1;
            let half = 1i64 << (bits - 1);
            let x: Vec<i32> = (0..n)
                .map(|i| {
                    s ^= s << 13;
                    s ^= s >> 7;
                    s ^= s << 17;
                    let v = if extreme {
                        if i % 2 == 0 { -half + 1 } else { half - 1 }
                    } else {
                        (s % (2 * half as u64)) as i64 - half
                    };
                    v.clamp(-(1 << 26) + 1, (1 << 26) - 1) as i32
                })
                .collect();
            proptest::prop_assert_eq!(abs_sums(&x), abs_sums_wide(&x));
        }
    }

    /// The definition: the order-`k` difference, by binomial coefficients, in `i128`.
    fn slow(x: &[i64], order: usize) -> Vec<i128> {
        let coef: [&[i128]; 5] = [&[], &[1], &[2, -1], &[3, -3, 1], &[4, -6, 4, -1]];
        (order..x.len())
            .map(|i| {
                let pred: i128 = coef[order]
                    .iter()
                    .enumerate()
                    .map(|(j, &c)| c * i128::from(x[i - 1 - j]))
                    .sum();
                i128::from(x[i]) - pred
            })
            .collect()
    }

    fn unfold(u: u32) -> i64 {
        i64::from((u >> 1) as i32 ^ -((u & 1) as i32))
    }

    #[test]
    fn matches_the_definition_on_both_paths() {
        let mut state = 99u64;
        let mut next = move || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            (state >> 20) as i64
        };
        for bits in [4u32, 16, 24, 27, 28, 31, 32, 33] {
            let x: Vec<i64> = (0..40).map(|_| next() % (1 << (bits - 1))).collect();
            for order in 0..=4 {
                let want = slow(&x, order);
                let fits = want
                    .iter()
                    .all(|&e| e > i128::from(i32::MIN) && e <= i128::from(i32::MAX));
                let mut out = Vec::new();
                let ok = if bits == 33 {
                    residual(&x, order, bits, &mut out)
                } else {
                    let x32: Vec<i32> = x.iter().map(|&v| v as i32).collect();
                    residual(&x32, order, bits, &mut out)
                };
                assert_eq!(ok, fits, "bits {bits} order {order}");
                if ok {
                    let got: Vec<i128> = out.iter().map(|&u| i128::from(unfold(u))).collect();
                    assert_eq!(got, want, "bits {bits} order {order}");
                }
            }
        }
    }

    #[test]
    fn residuals_beyond_32_bits_are_rejected() {
        let x = [i32::MIN, i32::MAX, i32::MIN, i32::MAX, i32::MIN];
        let mut out = Vec::new();
        assert!(!residual(&x, 1, 32, &mut out));
        assert!(
            !residual(&x, 0, 32, &mut out),
            "i32::MIN is not a legal residual"
        );
        assert!(residual(&x[1..2], 0, 32, &mut out));
    }
}
