//! Linear prediction (RFC 9639 section 9.2.9): window, autocorrelation, Levinson-Durbin,
//! coefficient quantisation and the residual.
//!
//! Floating point appears only here, and only through `+ - * /`, `sqrt`, comparisons and
//! conversions, which IEEE 754 defines exactly: no `sin`, `exp`, `ln` or fused multiply-add,
//! whose results differ between platform maths libraries. The same input therefore gives the
//! same bytes everywhere.

use super::analyze::Sample;
use super::fixed::fold;

pub(super) const MAX_ORDER: usize = 32;

/// A Tukey-like window: flat in the middle, with a smoothstep taper over the first and last
/// quarter. Smoothstep (`3x^2 - 2x^3`) stands in for the cosine taper, which would need `cos`.
pub(super) fn window(n: usize, out: &mut Vec<f64>) {
    out.clear();
    out.resize(n, 1.0);
    let taper = n / 4;
    for i in 0..taper {
        let x = (i as f64 + 0.5) / taper as f64;
        let w = x * x * (3.0 - 2.0 * x);
        out[i] = w;
        out[n - 1 - i] = w;
    }
}

/// `out[lag]` for lag 0 to `out.len() - 1`. Eight partial sums per lag, combined in a fixed
/// order, so the compiler can use vector units without changing the result.
pub(super) fn autocorrelation(data: &[f64], out: &mut [f64]) {
    for (lag, o) in out.iter_mut().enumerate() {
        if lag >= data.len() {
            *o = 0.0;
            continue;
        }
        let (a, b) = (&data[..data.len() - lag], &data[lag..]);
        let mut acc = [0.0f64; 8];
        let (ac, ar) = a.as_chunks::<8>();
        let (bc, br) = b.as_chunks::<8>();
        for (x, y) in ac.iter().zip(bc) {
            for k in 0..8 {
                acc[k] += x[k] * y[k];
            }
        }
        let mut tail = 0.0;
        for (x, y) in ar.iter().zip(br) {
            tail += x * y;
        }
        let low = (acc[0] + acc[1]) + (acc[2] + acc[3]);
        let high = (acc[4] + acc[5]) + (acc[6] + acc[7]);
        *o = (low + high) + tail;
    }
}

/// Levinson-Durbin recursion. `coefs[k][..=k]` are the order `k + 1` predictor coefficients
/// (`x[i] ~ sum coefs[k][j] * x[i - 1 - j]`) and `error[k]` the remaining error energy.
/// Returns how many orders it produced: it stops early when the energy runs out.
pub(super) fn levinson(
    autoc: &[f64],
    max_order: usize,
    coefs: &mut [[f64; MAX_ORDER]; MAX_ORDER],
    error: &mut [f64; MAX_ORDER],
) -> usize {
    let mut err = autoc[0];
    if err <= 0.0 {
        return 0;
    }
    let mut done = 0;
    for i in 0..max_order {
        let mut r = autoc[i + 1];
        for j in 0..i {
            r -= coefs[i.wrapping_sub(1)][j] * autoc[i - j];
        }
        r /= err;
        if !(-1.0..=1.0).contains(&r) {
            break;
        }
        let mut next = [0.0f64; MAX_ORDER];
        next[i] = r;
        for j in 0..i {
            next[j] = coefs[i - 1][j] - r * coefs[i - 1][i - 1 - j];
        }
        coefs[i] = next;
        err *= 1.0 - r * r;
        error[i] = err;
        done = i + 1;
        if err <= 0.0 {
            break;
        }
    }
    done
}

/// Rounds half away from zero without `round`.
fn round(x: f64) -> i64 {
    if x >= 0.0 {
        (x + 0.5) as i64
    } else {
        -((0.5 - x) as i64)
    }
}

/// Quantises `coefs` to `precision` bits (sign included) with the largest shift (at most 15)
/// that keeps them in range, carrying the rounding error from one coefficient to the next.
/// `None` when there is nothing to quantise.
pub(super) fn quantize(coefs: &[f64], precision: u32, out: &mut [i32]) -> Option<u32> {
    let qmax = (1i64 << (precision - 1)) - 1;
    let qmin = -(1i64 << (precision - 1));
    let cmax = coefs
        .iter()
        .fold(0.0f64, |m, &c| if c.abs() > m { c.abs() } else { m });
    if !(cmax > 0.0 && cmax.is_finite()) {
        return None;
    }
    let mut shift = 15u32;
    while shift > 0 && cmax * (1u64 << shift) as f64 > qmax as f64 {
        shift -= 1;
    }
    let scale = (1u64 << shift) as f64;
    let mut carry = 0.0;
    for (o, &c) in out.iter_mut().zip(coefs) {
        carry += c * scale;
        let q = round(carry).clamp(qmin, qmax);
        carry -= q as f64;
        *o = q as i32;
    }
    Some(shift)
}

/// log2 from the exponent and an `atanh` series for the mantissa; `+ - * /` only.
pub(super) fn log2(x: f64) -> f64 {
    debug_assert!(x > 0.0 && x.is_finite());
    let bits = x.to_bits();
    let exp = ((bits >> 52) & 0x7FF) as i64 - 1023;
    let m = f64::from_bits((bits & ((1 << 52) - 1)) | (1023 << 52));
    // ln m = 2 atanh z, z = (m - 1) / (m + 1), at most 1/3 for m in [1, 2).
    let z = (m - 1.0) / (m + 1.0);
    let z2 = z * z;
    let mut term = z;
    let mut sum = 0.0;
    let mut k = 1.0;
    for _ in 0..12 {
        sum += term / k;
        term *= z2;
        k += 2.0;
    }
    exp as f64 + 2.0 * sum * std::f64::consts::LOG2_E
}

/// A quantised predictor: `coefs[j]` multiplies the sample `j + 1` places back.
pub(super) struct Quantized<'a> {
    pub(super) coefs: &'a [i32],
    pub(super) shift: u32,
    pub(super) precision: u32,
}

/// Scratch sums for [`residual`], reused from block to block.
#[derive(Default)]
pub(super) struct Accumulators {
    narrow: Vec<i32>,
    wide: Vec<i64>,
}

impl Accumulators {
    pub(super) fn with_capacity(narrow: usize, wide: usize) -> Self {
        Self {
            narrow: Vec::with_capacity(narrow),
            wide: Vec::with_capacity(wide),
        }
    }

    #[cfg(test)]
    pub(super) fn capacities(&self) -> [usize; 2] {
        [self.narrow.capacity(), self.wide.capacity()]
    }
}

/// Fills `out` with the folded residuals of the quantised predictor over `x`
/// (`x.len() - coefs.len()` of them). False when a residual would not fit 32 bits.
///
/// The outer loop runs over coefficients and the inner one over samples, which is the shape
/// that vectorises.
pub(super) fn residual<T: Sample>(
    x: &[T],
    q: &Quantized,
    bps: u32,
    out: &mut Vec<u32>,
    acc: &mut Accumulators,
) -> bool {
    let (coefs, shift, precision) = (q.coefs, q.shift, q.precision);
    let (acc32, acc64) = (&mut acc.narrow, &mut acc.wide);
    let order = coefs.len();
    let m = x.len() - order;
    out.clear();
    // Largest possible sum: order * 2^(precision-1) * 2^(bps-1) stays below 2^31.
    let narrow = bps + precision + (usize::BITS - (order - 1).leading_zeros()) <= 32;
    if narrow {
        acc32.clear();
        let first = &x[order - 1..order - 1 + m];
        acc32.extend(first.iter().map(|&v| v.low32().wrapping_mul(coefs[0])));
        for (j, &c) in coefs.iter().enumerate().skip(1) {
            let src = &x[order - 1 - j..order - 1 - j + m];
            for (a, &v) in acc32.iter_mut().zip(src) {
                *a = a.wrapping_add(v.low32().wrapping_mul(c));
            }
        }
        out.extend(
            acc32
                .iter()
                .zip(&x[order..])
                .map(|(&a, &v)| fold(v.low32().wrapping_sub(a >> shift))),
        );
        return true;
    }
    acc64.clear();
    let c0 = i64::from(coefs[0]);
    acc64.extend(x[order - 1..order - 1 + m].iter().map(|&v| v.into() * c0));
    for (j, &c) in coefs.iter().enumerate().skip(1) {
        let src = &x[order - 1 - j..order - 1 - j + m];
        let c = i64::from(c);
        for (a, &v) in acc64.iter_mut().zip(src) {
            *a += v.into() * c;
        }
    }
    let mut bad = false;
    out.extend(acc64.iter().zip(&x[order..]).map(|(&a, &v)| {
        let e = v.into() - (a >> shift);
        bad |= e <= i64::from(i32::MIN) || e > i64::from(i32::MAX);
        fold(e as i32)
    }));
    !bad
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unfold(u: u32) -> i128 {
        i128::from((u >> 1) as i32 ^ -((u & 1) as i32))
    }

    #[test]
    fn residual_matches_the_definition() {
        let mut state = 5u64;
        let mut next = move |bits: u32| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((state >> 11) as i64) % (1 << (bits - 1))
        };
        let mut acc = Accumulators::default();
        for (bits, precision) in [(8u32, 7u32), (16, 12), (24, 13), (32, 15), (33, 9)] {
            for order in [1usize, 2, 5, 8, 12, 32] {
                let x: Vec<i64> = (0..70).map(|_| next(bits)).collect();
                let coefs: Vec<i32> = (0..order)
                    .map(|_| (next(precision + 1) % (1 << (precision - 1))) as i32)
                    .collect();
                for shift in [0u32, 5, 15] {
                    let want: Vec<i128> = (order..x.len())
                        .map(|i| {
                            let sum: i128 = (0..order)
                                .map(|j| i128::from(coefs[j]) * i128::from(x[i - 1 - j]))
                                .sum();
                            i128::from(x[i]) - (sum >> shift)
                        })
                        .collect();
                    let fits = want
                        .iter()
                        .all(|&e| e > i128::from(i32::MIN) && e <= i128::from(i32::MAX));
                    let mut out = Vec::new();
                    let ok = if bits == 33 {
                        residual(
                            &x,
                            &Quantized {
                                coefs: &coefs,
                                shift,
                                precision,
                            },
                            bits,
                            &mut out,
                            &mut acc,
                        )
                    } else {
                        let x32: Vec<i32> = x.iter().map(|&v| v as i32).collect();
                        residual(
                            &x32,
                            &Quantized {
                                coefs: &coefs,
                                shift,
                                precision,
                            },
                            bits,
                            &mut out,
                            &mut acc,
                        )
                    };
                    assert_eq!(ok, fits, "bits {bits} order {order} shift {shift}");
                    if ok {
                        let got: Vec<i128> = out.iter().map(|&u| unfold(u)).collect();
                        assert_eq!(got, want, "bits {bits} order {order} shift {shift}");
                    }
                }
            }
        }
    }

    #[test]
    fn window_is_symmetric_flat_in_the_middle_and_tapered() {
        let mut w = Vec::new();
        for n in [1usize, 3, 16, 17, 4096] {
            window(n, &mut w);
            assert_eq!(w.len(), n);
            for i in 0..n {
                assert_eq!(w[i], w[n - 1 - i]);
                assert!((0.0..=1.0).contains(&w[i]));
            }
        }
        window(4096, &mut w);
        assert!(w[0] < 1e-5 && w[2048] == 1.0 && w[1023] < 1.0 && w[1024] == 1.0);
        assert!(w.windows(2).take(1024).all(|p| p[0] <= p[1]));
    }

    #[test]
    fn autocorrelation_is_the_lagged_dot_product() {
        let data: Vec<f64> = (0..37).map(|i| f64::from(i * i % 11) - 5.0).collect();
        let mut got = [0.0; 9];
        autocorrelation(&data, &mut got);
        for (lag, &g) in got.iter().enumerate() {
            let want: f64 = (lag..data.len()).map(|i| data[i] * data[i - lag]).sum();
            assert_eq!(g, want, "lag {lag}"); // integer-valued, so exact in any order
        }
        let mut short = [0.0; 4];
        autocorrelation(&data[..2], &mut short);
        assert_eq!(short[2], 0.0);
    }

    #[test]
    fn levinson_recovers_a_first_order_process() {
        let rho = 0.5;
        let autoc: Vec<f64> = (0..6).map(|k| 4.0 * f64::powi(rho, k)).collect();
        let (mut c, mut e) = ([[0.0; MAX_ORDER]; MAX_ORDER], [0.0; MAX_ORDER]);
        let orders = levinson(&autoc, 5, &mut c, &mut e);
        assert_eq!(orders, 5);
        assert!((c[0][0] - 0.5).abs() < 1e-12);
        assert!((c[1][0] - 0.5).abs() < 1e-12 && c[1][1].abs() < 1e-12);
        assert!((e[0] - 3.0).abs() < 1e-12);
        let (mut c, mut e) = ([[0.0; MAX_ORDER]; MAX_ORDER], [0.0; MAX_ORDER]);
        assert_eq!(
            levinson(&[0.0; 5], 4, &mut c, &mut e),
            0,
            "silence has no predictor"
        );
    }

    #[test]
    fn quantisation_keeps_range_shift_and_accuracy() {
        let coefs = [1.9, -0.95, 0.0123, -0.0002];
        let mut q = [0i32; 4];
        for precision in 5..=15 {
            let shift = quantize(&coefs, precision, &mut q).unwrap();
            assert!(shift <= 15);
            let lim = 1i32 << (precision - 1);
            assert!(q.iter().all(|&v| v >= -lim && v < lim));
            let scale = f64::from(1u32 << shift);
            // The error carries from one coefficient to the next, so it stays under a step.
            let drift: f64 = coefs
                .iter()
                .zip(&q)
                .map(|(&c, &v)| c * scale - f64::from(v))
                .sum();
            assert!(drift.abs() <= 0.5 + 1e-9, "p {precision}: {drift}");
        }
        assert_eq!(quantize(&[0.0, 0.0], 10, &mut q), None);
        assert_eq!(quantize(&[f64::NAN], 10, &mut q), None);
        // Huge coefficients are clamped at shift 0.
        assert_eq!(quantize(&[1e9], 8, &mut q), Some(0));
        assert_eq!(q[0], 127);
    }

    #[test]
    fn log2_is_accurate_and_monotone() {
        for (x, want) in [
            (1.0, 0.0),
            (2.0, 1.0),
            (0.5, -1.0),
            (1024.0, 10.0),
            (3.0, 1.584_962_500_721_156),
        ] {
            assert!((log2(x) - want).abs() < 1e-9, "{x}");
        }
        assert!(log2(1.999) < log2(2.0) && log2(2.0) < log2(2.001));
    }
}
