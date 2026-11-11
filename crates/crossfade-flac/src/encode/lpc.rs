//! Linear prediction (RFC 9639 section 9.2.9): window, autocorrelation, Levinson-Durbin,
//! coefficient quantisation and refinement, the covariance that scores a predictor without
//! coding it, and the residual.
//!
//! Floating point appears only here (and in the analysis's size estimates), and only through
//! `+ - * /`, `sqrt`, comparisons and
//! conversions, which IEEE 754 defines exactly: no `sin`, `exp`, `ln` or fused multiply-add,
//! whose results differ between platform maths libraries. The same input therefore gives the
//! same bytes everywhere.

use super::analyze::Sample;
use super::fixed::fold;

pub(super) const MAX_ORDER: usize = 32;

/// A Tukey-like window over `from..to` of `n` samples, zero outside: flat in the middle with
/// a smoothstep taper over the first and last quarter of its span. Smoothstep (`3x^2 - 2x^3`)
/// stands in for the cosine taper, which would need `cos`.
pub(super) fn window(n: usize, from: usize, to: usize, div: usize, out: &mut Vec<f64>) {
    out.clear();
    out.resize(n, 0.0);
    let (from, to) = (from.min(n), to.min(n));
    out[from..to].fill(1.0);
    let taper = (to - from) / div;
    for i in 0..taper {
        let x = (i as f64 + 0.5) / taper as f64;
        let w = x * x * (3.0 - 2.0 * x);
        out[from + i] = w;
        out[to - 1 - i] = w;
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

/// Scratch for [`windowed_autocorrelation`].
pub(super) struct Windowed<'a> {
    pub(super) float: &'a mut Vec<f64>,
    pub(super) int: &'a mut Vec<i32>,
}

/// Fractional bits the windowed samples keep when the autocorrelation runs on integers: as
/// many as `n` products of samples up to `peak` (times 2^bits) can sum in an `i64` without
/// overflow, at most 20. `None` when not even the integer part fits.
pub(super) fn int_scale(n: usize, peak: u64) -> Option<u32> {
    let log_n = usize::BITS - n.saturating_sub(1).leading_zeros();
    let width = 64 - peak.leading_zeros();
    // |v| <= 2^(width + bits), so n products sum to at most 2^(2 (width + bits) + log_n).
    let room = 62u32.checked_sub(log_n)? / 2;
    room.checked_sub(width).map(|bits| bits.min(20))
}

/// The autocorrelation of `x` under `window` (lags `0..out.len()`). The windowed samples are
/// rounded to `int_scale` fractional bits and their lagged products summed exactly in `i64`,
/// so the order of the sums is free and the compiler vectorises them; samples too wide for
/// that take the floating-point path.
pub(super) fn windowed_autocorrelation<T: Sample>(
    x: &[T],
    window: &[f64],
    peak: u64,
    scratch: &mut Windowed,
    out: &mut [f64],
) {
    let Some(bits) = int_scale(x.len(), peak) else {
        scratch.float.clear();
        scratch
            .float
            .extend(x.iter().zip(window).map(|(&v, &w)| v.into() as f64 * w));
        autocorrelation(scratch.float, out);
        return;
    };
    let scale = (1u64 << bits) as f64;
    let v = &mut *scratch.int;
    v.clear();
    v.extend(
        x.iter()
            .zip(window)
            .map(|(&s, &w)| round(s.into() as f64 * w * scale) as i32),
    );
    let mut sums = [0i64; MAX_ORDER + 1];
    lagged_sums(v, &mut sums[..out.len()]);
    let unscale = 1.0 / (1u64 << (2 * bits)) as f64;
    for (o, &s) in out.iter_mut().zip(&sums) {
        *o = s as f64 * unscale;
    }
}

/// `out[lag]` is the sum of `v[t] * v[t + lag]`, exact as long as it fits an `i64` (the
/// caller's bound). Pads `v` with zeros past its end.
fn lagged_sums(v: &mut Vec<i32>, out: &mut [i64]) {
    let n = v.len();
    // Zeros past the end let every lag run over the same range.
    v.resize(n + out.len() + 4, 0);
    // Four lags per pass: four independent sums, and each sample loaded once for them.
    for (group, o) in out.chunks_mut(4).enumerate() {
        let lag = group * 4;
        let (a, b) = (&v[..n], &v[lag..lag + n + 3]);
        let mut acc = [0i64; 4];
        for (i, &p) in a.iter().enumerate() {
            let q: &[i32; 4] = b[i..i + 4].try_into().unwrap_or(&[0; 4]);
            for k in 0..4 {
                acc[k] += i64::from(p) * i64::from(q[k]);
            }
        }
        o.copy_from_slice(&acc[..o.len()]);
    }
    v.truncate(n);
}

/// The covariance of a block for predictors up to order `p`: `phi[a][b]` is the sum over
/// `i` in `p..n` of `x[i - a] * x[i - b]`, exact in `i64`. With it the error energy of any
/// predictor of order up to `p` over those samples is a quadratic form, a few hundred
/// operations instead of a pass over the block.
pub(super) struct Covariance {
    p: usize,
    n: usize,
    phi: [[i64; MAX_ORDER + 1]; MAX_ORDER + 1],
}

impl Covariance {
    pub(super) fn new() -> Self {
        Self {
            p: 0,
            n: 0,
            phi: [[0; MAX_ORDER + 1]; MAX_ORDER + 1],
        }
    }

    /// Fills the matrix for `x` and order `p`; false when `x` is not `i32` samples whose
    /// sums are sure to fit an `i64`. `scratch` is reused.
    pub(super) fn compute<T: Sample>(
        &mut self,
        x: &[T],
        p: usize,
        peak: u64,
        scratch: &mut Vec<i32>,
    ) -> bool {
        let n = x.len();
        let Some(x) = T::as_i32(x) else {
            return false;
        };
        if p >= n || int_scale(n, peak).is_none() {
            return false;
        }
        (self.p, self.n) = (p, n);
        scratch.clear();
        scratch.extend_from_slice(x);
        let mut full = [0i64; MAX_ORDER + 1];
        lagged_sums(scratch, &mut full[..=p]);
        // The whole-block sums with the ends trimmed: for entry (a, b), t = i - b runs over
        // p - b..n - b, where the lagged sum covers 0..n - d.
        let v = |t: usize| i64::from(x[t]);
        for a in 0..=p {
            for b in a..=p {
                let d = b - a;
                let head: i64 = (0..p - b).map(|t| v(t) * v(t + d)).sum();
                let tail: i64 = (n - b..n - d).map(|t| v(t) * v(t + d)).sum();
                let e = full[d] - head - tail;
                self.phi[a][b] = e;
                self.phi[b][a] = e;
            }
        }
        true
    }

    /// The error energy, times `2^(2 shift)`, of the quantised predictor `q` over samples
    /// `p..n` before the prediction is floored: exact.
    pub(super) fn scaled_energy(&self, q: &[i32], shift: u32) -> i128 {
        let m = q.len();
        debug_assert!(m <= self.p);
        let phi = &self.phi;
        let mut cross: i128 = 0;
        let mut quad: i128 = 0;
        for j in 0..m {
            let qj = i128::from(q[j]);
            cross += qj * i128::from(phi[0][j + 1]);
            let mut row: i128 = 0;
            for k in 0..m {
                row += i128::from(q[k]) * i128::from(phi[j + 1][k + 1]);
            }
            quad += qj * row;
        }
        (i128::from(phi[0][0]) << (2 * shift)) - (cross << (shift + 1)) + quad
    }

    /// Mean squared residual per sample of `q`: the exact energy, plus a third for the
    /// flooring of each prediction (a uniform `[0, 1)` added to every residual).
    pub(super) fn mean_energy(&self, q: &[i32], shift: u32) -> f64 {
        let count = (self.n - self.p) as f64;
        let e = self.scaled_energy(q, shift).max(0) as f64 / (1u64 << (2 * shift)) as f64;
        e / count + 1.0 / 3.0
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

/// Improves quantised coefficients by coordinate descent on the prediction error they add: with
/// `d` the difference between the real and the quantised coefficients (in coefficient units),
/// the extra residual energy is `d' R d` for the autocorrelation matrix `R`. Rounding each
/// coefficient to its nearest integer is not the minimum of that form when the signal is highly
/// predictable and the matrix badly conditioned, where it matters: one more bit of residual per
/// sample for every halving of the coefficients' accuracy. Steps of one unit, passes over every
/// coefficient, at most `passes` of them.
///
/// Each move's gain adds the same floating-point terms in the same order as the plain
/// definition (kept as a test reference), with the products that do not change from move to
/// move computed once, so the result is identical and several times cheaper.
#[allow(clippy::needless_range_loop)]
pub(super) fn refine(
    coefs: &[f64],
    q: &mut [i32],
    shift: u32,
    precision: u32,
    autoc: &[f64],
    passes: usize,
) {
    let order = coefs.len();
    let scale = (1u64 << shift) as f64;
    let (qmin, qmax) = (-(1i64 << (precision - 1)), (1i64 << (precision - 1)) - 1);
    let mut d = [0.0f64; MAX_ORDER];
    for i in 0..order {
        d[i] = coefs[i] - f64::from(q[i]) / scale;
    }
    // g = R d, kept current as `d` changes.
    let mut g = [0.0f64; MAX_ORDER];
    let at = |i: usize, j: usize| autoc[i.abs_diff(j)];
    for i in 0..order {
        let mut sum = 0.0;
        for j in 0..order {
            sum += at(i, j) * d[j];
        }
        g[i] = sum;
    }
    let step = 1.0 / scale;
    let limit = -1e-12 * autoc[0] * step * step;
    // A move's terms: (2 u_a step) g and (u_a step)(u_b step) R; the signs are exact, so
    // the magnitudes `two_step * g` and `pair[|a - b|]` are the same products.
    let two_step = 2.0 * step;
    let s2 = step * step;
    let lag = |k: usize| autoc.get(k).copied().unwrap_or(0.0);
    let pair = [s2 * lag(0), s2 * lag(1), s2 * lag(2)];
    let [p0, p1, p2] = pair;
    for _ in 0..passes {
        let mut changed = false;
        for width in 1..=3usize.min(order) {
            for i in 0..=order - width {
                for pat in 0..1usize << width {
                    if width == 3 && (pat == 0 || pat == 7) {
                        continue;
                    }
                    // The sign of each coefficient's term: its unit moves by u, the term by -u.
                    let mut u = [0i64; 3];
                    for (k, v) in u.iter_mut().enumerate().take(width) {
                        *v = if pat >> k & 1 == 1 { 1 } else { -1 };
                    }
                    let [s0, s1, s2] = u.map(|v| -(v as f64));
                    let a = |k: usize| two_step * g[i + k];
                    // Term by term in the definition's order: for each a, its g term, then
                    // its R terms for b = 0, 1, 2.
                    let gain = match width {
                        1 => 0.0 + s0 * a(0) + p0,
                        2 => 0.0 + s0 * a(0) + p0 + s0 * s1 * p1 + s1 * a(1) + s1 * s0 * p1 + p0,
                        _ => {
                            0.0 + s0 * a(0)
                                + p0
                                + s0 * s1 * p1
                                + s0 * s2 * p2
                                + s1 * a(1)
                                + s1 * s0 * p1
                                + p0
                                + s1 * s2 * p1
                                + s2 * a(2)
                                + s2 * s0 * p2
                                + s2 * s1 * p1
                                + p0
                        }
                    };
                    if gain < limit
                        && (0..width).all(|a| {
                            let m = i64::from(q[i + a]) + u[a];
                            m >= qmin && m <= qmax
                        })
                    {
                        for a in 0..width {
                            q[i + a] += u[a] as i32;
                            d[i + a] -= u[a] as f64 * step;
                        }
                        for j in 0..order {
                            let mut sum = 0.0;
                            for a in 0..width {
                                sum += at(j, i + a) * (-(u[a] as f64) * step);
                            }
                            g[j] += sum;
                        }
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
}

/// [`refine`] as first written, term by term: the fast version must match it exactly.
/// Improves quantised coefficients by coordinate descent on the prediction error they add: with
/// `d` the difference between the real and the quantised coefficients (in coefficient units),
/// the extra residual energy is `d' R d` for the autocorrelation matrix `R`. Rounding each
/// coefficient to its nearest integer is not the minimum of that form when the signal is highly
/// predictable and the matrix badly conditioned, where it matters: one more bit of residual per
/// sample for every halving of the coefficients' accuracy. Steps of one unit, passes over every
/// coefficient, at most `passes` of them.
#[cfg(test)]
#[allow(clippy::needless_range_loop)]
fn refine_simple(
    coefs: &[f64],
    q: &mut [i32],
    shift: u32,
    precision: u32,
    autoc: &[f64],
    passes: usize,
) {
    let order = coefs.len();
    let scale = (1u64 << shift) as f64;
    let (qmin, qmax) = (-(1i64 << (precision - 1)), (1i64 << (precision - 1)) - 1);
    let mut d = [0.0f64; MAX_ORDER];
    for i in 0..order {
        d[i] = coefs[i] - f64::from(q[i]) / scale;
    }
    // g = R d, kept current as `d` changes.
    let mut g = [0.0f64; MAX_ORDER];
    let at = |i: usize, j: usize| autoc[i.abs_diff(j)];
    for i in 0..order {
        let mut sum = 0.0;
        for j in 0..order {
            sum += at(i, j) * d[j];
        }
        g[i] = sum;
    }
    let step = 1.0 / scale;
    // Moves: single units, and pairs and triples of neighbouring coefficients moving together
    // (a signal's coefficients are strongly correlated, so the cheapest corrections are).
    for _ in 0..passes {
        let mut changed = false;
        for width in 1..=3usize.min(order) {
            for i in 0..=order - width {
                for pat in 0..1usize << width {
                    // Direction of each coefficient in the move, from the bits of `pat`; the
                    // all-same-sign moves cover the smooth corrections too.
                    let mut u = [0i64; 3];
                    for (k, v) in u.iter_mut().enumerate().take(width) {
                        *v = if pat >> k & 1 == 1 { 1 } else { -1 };
                    }
                    if width == 3 && (pat == 0 || pat == 7) {
                        continue;
                    }
                    let mut gain = 0.0;
                    for a in 0..width {
                        let ua = -(u[a] as f64) * step;
                        gain += 2.0 * ua * g[i + a];
                        for b in 0..width {
                            let ub = -(u[b] as f64) * step;
                            gain += ua * ub * at(i + a, i + b);
                        }
                    }
                    let fits = (0..width).all(|a| {
                        let m = i64::from(q[i + a]) + u[a];
                        m >= qmin && m <= qmax
                    });
                    if gain < -1e-12 * autoc[0] * step * step && fits {
                        for a in 0..width {
                            q[i + a] += u[a] as i32;
                            d[i + a] -= u[a] as f64 * step;
                        }
                        for j in 0..order {
                            let mut sum = 0.0;
                            for a in 0..width {
                                sum += at(j, i + a) * (-(u[a] as f64) * step);
                            }
                            g[j] += sum;
                        }
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
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
    let order = coefs.len();
    // Largest possible sum: order * 2^(precision-1) * 2^(bps-1) stays below 2^31.
    let narrow = bps + precision + (usize::BITS - (order - 1).leading_zeros()) <= 32;
    if let Some(x) = T::as_i32(x) {
        macro_rules! by_order {
            ($($n:literal)*) => {
                match order {
                    $($n => {
                        let c: &[i32; $n] = coefs.try_into().unwrap_or(&[0; $n]);
                        return if narrow {
                            kernel::<i32, $n>(x, c, shift, out)
                        } else {
                            kernel::<i64, $n>(x, c, shift, out)
                        };
                    })*
                    _ => {}
                }
            };
        }
        by_order!(1 2 3 4 5 6 7 8 9 10 11 12);
    }
    residual_generic(x, q, narrow, out, acc)
}

/// The accumulator of a residual kernel: `i32` when the bound proves the sum fits, else `i64`.
trait Acc: Copy {
    const ZERO: Self;
    /// The residual can be illegal (outside 32 bits, or `i32::MIN`), so it must be checked.
    const CHECK: bool;
    fn mac(self, c: i32, x: i32) -> Self;
    /// `target - (self >> shift)`, wide enough to check.
    fn residual(self, target: i32, shift: u32) -> i64;
}

impl Acc for i32 {
    const ZERO: Self = 0;
    // The caller proved the sum fits, and then the difference is legal too.
    const CHECK: bool = false;
    #[inline(always)]
    fn mac(self, c: i32, x: i32) -> Self {
        self.wrapping_add(c.wrapping_mul(x))
    }
    #[inline(always)]
    fn residual(self, target: i32, shift: u32) -> i64 {
        i64::from(target.wrapping_sub(self >> shift))
    }
}

impl Acc for i64 {
    const ZERO: Self = 0;
    const CHECK: bool = true;
    #[inline(always)]
    fn mac(self, c: i32, x: i32) -> Self {
        self + i64::from(c) * i64::from(x)
    }
    #[inline(always)]
    fn residual(self, target: i32, shift: u32) -> i64 {
        i64::from(target) - (self >> shift)
    }
}

/// One order, sample by sample, with the coefficients unrolled: the shape the compiler turns
/// into one vector multiply-add per coefficient for several samples at once.
fn kernel<A: Acc, const N: usize>(x: &[i32], c: &[i32; N], shift: u32, out: &mut Vec<u32>) -> bool {
    let m = x.len() - N;
    out.clear();
    out.resize(m, 0);
    // rc[k] multiplies the sample k places after the start of the window.
    let mut rc = [0i32; N];
    for (j, &v) in c.iter().enumerate() {
        rc[N - 1 - j] = v;
    }
    let (mut lo, mut hi) = (0i64, 0i64);
    for (w, o) in x.windows(N + 1).zip(out.iter_mut()) {
        let mut sum = A::ZERO;
        for k in 0..N {
            sum = sum.mac(rc[k], w[k]);
        }
        let e = sum.residual(w[N], shift);
        if A::CHECK {
            lo = lo.min(e);
            hi = hi.max(e);
        }
        *o = fold(e as i32);
    }
    lo > i64::from(i32::MIN) && hi <= i64::from(i32::MAX)
}

/// Any order and sample type: coefficients outer, samples inner.
fn residual_generic<T: Sample>(
    x: &[T],
    q: &Quantized,
    narrow: bool,
    out: &mut Vec<u32>,
    acc: &mut Accumulators,
) -> bool {
    let (coefs, shift) = (q.coefs, q.shift);
    let (acc32, acc64) = (&mut acc.narrow, &mut acc.wide);
    let order = coefs.len();
    let m = x.len() - order;
    out.clear();
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

    proptest::proptest! {
        /// The order-specialised kernels equal the general loop, on both sides of the bound
        /// that picks 32-bit sums, at the extremes of every range.
        #[test]
        fn kernels_equal_the_general_loop(
            order in 1usize..=12,
            bps in 4u32..=32,
            precision in 5u32..=15,
            shift in 0u32..=15,
            seed in proptest::prelude::any::<u64>(),
            extreme in 0u8..4,
        ) {
            let mut state = seed | 1;
            let mut next = move || {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state
            };
            let (lo, hi) = (-(1i64 << (bps - 1)), (1i64 << (bps - 1)) - 1);
            let x: Vec<i32> = (0..order + 40)
                .map(|_| match extreme {
                    0 => lo + (next() % (hi - lo + 1) as u64) as i64,
                    1 => if next() & 1 == 0 { lo } else { hi },
                    _ => (next() % 7) as i64 - 3,
                } as i32)
                .collect();
            let (cmin, cmax) = (-(1i32 << (precision - 1)), (1i32 << (precision - 1)) - 1);
            let coefs: Vec<i32> = (0..order)
                .map(|_| if extreme == 1 {
                    if next() & 1 == 0 { cmin } else { cmax }
                } else {
                    cmin + (next() % (i64::from(cmax) - i64::from(cmin) + 1) as u64) as i32
                })
                .collect();
            let q = Quantized { coefs: &coefs, shift, precision };
            let narrow = bps + precision + (usize::BITS - (order - 1).leading_zeros()) <= 32;
            let mut acc = Accumulators::default();
            let (mut fast, mut slow) = (Vec::new(), Vec::new());
            let ok_fast = residual(&x, &q, bps, &mut fast, &mut acc);
            let ok_slow = residual_generic(&x, &q, narrow, &mut slow, &mut acc);
            proptest::prop_assert_eq!(ok_fast, ok_slow);
            if ok_slow {
                proptest::prop_assert_eq!(fast, slow);
            }
        }
    }

    #[test]
    fn window_is_symmetric_flat_in_the_middle_and_tapered() {
        let mut w = Vec::new();
        for n in [1usize, 3, 16, 17, 4096] {
            window(n, 0, n, 4, &mut w);
            assert_eq!(w.len(), n);
            for i in 0..n {
                assert_eq!(w[i], w[n - 1 - i]);
                assert!((0.0..=1.0).contains(&w[i]));
            }
        }
        window(4096, 0, 4096, 4, &mut w);
        assert!(w[0] < 1e-5 && w[2048] == 1.0 && w[1023] < 1.0 && w[1024] == 1.0);
        assert!(w.windows(2).take(1024).all(|p| p[0] <= p[1]));
        // A partial window is zero outside its span and tapers inside it.
        window(4096, 1024, 3072, 4, &mut w);
        assert!(w[..1024].iter().all(|&v| v == 0.0) && w[3072..].iter().all(|&v| v == 0.0));
        assert!(w[1024] < 1e-4 && w[1536] == 1.0 && w[3071] < 1e-4);
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
    fn integer_scale_keeps_every_sum_inside_i64() {
        for n in [1usize, 2, 17, 1152, 4096, 4097, 65535] {
            let log_n = usize::BITS - (n - 1).leading_zeros();
            for width in 0u32..=40 {
                let peak = if width == 0 { 0 } else { (1u64 << width) - 1 };
                match int_scale(n, peak) {
                    Some(bits) => {
                        assert!(bits <= 20);
                        assert!(2 * (width + bits) + log_n <= 62, "n {n} width {width}");
                        // As many fractional bits as fit, up to the cap.
                        assert!(bits == 20 || 2 * (width + bits + 1) + log_n > 62);
                    }
                    None => assert!(2 * width + log_n > 62, "n {n} width {width}"),
                }
            }
        }
    }

    /// The exact autocorrelation of the windowed samples rounded to `bits` fractional bits.
    fn exact(x: &[i32], w: &[f64], bits: u32, lags: usize) -> Vec<f64> {
        let scale = (1u64 << bits) as f64;
        let q: Vec<i128> = x
            .iter()
            .zip(w)
            .map(|(&s, &w)| i128::from(round(f64::from(s) * w * scale)))
            .collect();
        (0..lags)
            .map(|lag| {
                let sum: i128 = (lag..q.len()).map(|i| q[i] * q[i - lag]).sum();
                sum as f64 / (1u64 << (2 * bits)) as f64
            })
            .collect()
    }

    #[test]
    fn windowed_autocorrelation_is_exact_in_integers() {
        let mut state = 77u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let (mut float, mut int) = (Vec::new(), Vec::new());
        let mut w = Vec::new();
        for (n, depth) in [
            (4096usize, 16u32),
            (4096, 17),
            (4096, 24),
            (4096, 25),
            (1152, 8),
            (37, 20),
            (3, 4),
        ] {
            window(n, 0, n, 4, &mut w);
            let lo = -(1i64 << (depth - 1));
            let signals: [Vec<i32>; 3] = [
                (0..n)
                    .map(|_| (lo + (next() % (1u64 << depth)) as i64) as i32)
                    .collect(),
                // Every sample at the extreme: the largest sums the bound must hold.
                vec![lo as i32; n],
                (0..n)
                    .map(|i| {
                        if i % 2 == 0 {
                            lo as i32
                        } else {
                            (-lo - 1) as i32
                        }
                    })
                    .collect(),
            ];
            for x in &signals {
                let peak = x
                    .iter()
                    .map(|&v| u64::from(v.unsigned_abs()))
                    .max()
                    .unwrap();
                let bits = int_scale(n, peak).expect("these depths take the integer path");
                let mut got = [0.0f64; 13];
                windowed_autocorrelation(
                    x,
                    &w,
                    peak,
                    &mut Windowed {
                        float: &mut float,
                        int: &mut int,
                    },
                    &mut got,
                );
                assert_eq!(got.to_vec(), exact(x, &w, bits, 13), "n {n} depth {depth}");
            }
        }
    }

    #[test]
    fn samples_too_wide_for_integers_take_the_float_path() {
        let n = 4096;
        let mut w = Vec::new();
        window(n, 0, n, 4, &mut w);
        let x: Vec<i64> = (0..n as i64)
            .map(|i| (i * 7919 % 4001 - 2000) << 30)
            .collect();
        let peak = x.iter().map(|&v| v.unsigned_abs()).max().unwrap();
        assert_eq!(int_scale(n, peak), None);
        let (mut float, mut int) = (Vec::new(), Vec::new());
        let mut got = [0.0f64; 9];
        windowed_autocorrelation(
            &x,
            &w,
            peak,
            &mut Windowed {
                float: &mut float,
                int: &mut int,
            },
            &mut got,
        );
        let windowed: Vec<f64> = x.iter().zip(&w).map(|(&v, &w)| v as f64 * w).collect();
        let mut want = [0.0f64; 9];
        autocorrelation(&windowed, &mut want);
        assert_eq!(got, want);
    }

    #[test]
    fn covariance_energy_is_the_residual_energy_exactly() {
        let mut state = 99u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut scratch = Vec::new();
        for (n, depth, p) in [
            (4096usize, 16u32, 12usize),
            (4096, 24, 8),
            (1000, 25, 12),
            (40, 8, 3),
        ] {
            let lo = -(1i64 << (depth - 1));
            let signals: [Vec<i32>; 2] = [
                (0..n)
                    .map(|_| (lo + (next() % (1u64 << depth)) as i64) as i32)
                    .collect(),
                // At the extreme, where the sums are largest.
                (0..n)
                    .map(|i| {
                        if i % 3 == 0 {
                            lo as i32
                        } else {
                            (-lo - 1) as i32
                        }
                    })
                    .collect(),
            ];
            for x in &signals {
                let peak = x
                    .iter()
                    .map(|&v| u64::from(v.unsigned_abs()))
                    .max()
                    .unwrap();
                let mut cov = Covariance::new();
                assert!(cov.compute(x, p, peak, &mut scratch));
                for m in [1, p / 2 + 1, p] {
                    for shift in [0u32, 9, 15] {
                        let q: Vec<i32> = (0..m).map(|_| (next() % 32768) as i32 - 16384).collect();
                        let want: i128 = (p..n)
                            .map(|i| {
                                let pred: i128 = (0..m)
                                    .map(|j| i128::from(q[j]) * i128::from(x[i - 1 - j]))
                                    .sum();
                                let e = (i128::from(x[i]) << shift) - pred;
                                e * e
                            })
                            .sum();
                        assert_eq!(
                            cov.scaled_energy(&q, shift),
                            want,
                            "n {n} depth {depth} m {m}"
                        );
                    }
                }
            }
        }
        // Too wide for exact sums, or not i32 samples: no screening.
        let wide = vec![i32::MIN, i32::MAX, 5, 7];
        assert!(!Covariance::new().compute(&wide, 2, 1 << 31, &mut scratch));
        assert!(!Covariance::new().compute(&[1i64, 2, 3, 4], 2, 4, &mut scratch));
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

    /// The added error energy of quantised coefficients: d' R d with d the difference.
    fn added_error(coefs: &[f64], q: &[i32], shift: u32, autoc: &[f64]) -> f64 {
        let scale = f64::from(1u32 << shift);
        let d: Vec<f64> = coefs
            .iter()
            .zip(q)
            .map(|(&c, &v)| c - f64::from(v) / scale)
            .collect();
        let mut total = 0.0;
        for i in 0..d.len() {
            for j in 0..d.len() {
                total += d[i] * d[j] * autoc[i.abs_diff(j)];
            }
        }
        total
    }

    #[test]
    fn refinement_never_adds_error_and_keeps_the_range() {
        // A narrow-band signal: autocorrelation of a few sines, so the matrix is ill-conditioned.
        let n = 2048;
        let x: Vec<f64> = (0..n)
            .map(|t| {
                let t = f64::from(t);
                (t * 0.05).sin() * 3000.0 + (t * 0.13).sin() * 1500.0 + (t * 0.31).sin() * 700.0
            })
            .collect();
        let mut autoc = [0.0; 9];
        autocorrelation(&x, &mut autoc);
        autoc[0] *= 1.0 + 1e-12;
        let (mut c, mut e) = ([[0.0; MAX_ORDER]; MAX_ORDER], [0.0; MAX_ORDER]);
        let orders = levinson(&autoc, 8, &mut c, &mut e);
        for order in 2..=orders {
            for precision in [8u32, 12, 15] {
                let coefs = &c[order - 1][..order];
                let mut q = vec![0i32; order];
                let shift = quantize(coefs, precision, &mut q).unwrap();
                let before = added_error(coefs, &q, shift, &autoc);
                refine(coefs, &mut q, shift, precision, &autoc, 8);
                let after = added_error(coefs, &q, shift, &autoc);
                assert!(
                    after <= before * (1.0 + 1e-9),
                    "order {order} p {precision}: {before} -> {after}"
                );
                let lim = 1i32 << (precision - 1);
                assert!(q.iter().all(|&v| v >= -lim && v < lim));
            }
        }
    }

    proptest::proptest! {
        /// The refinement with its products hoisted takes the same steps as the plain one.
        #[test]
        fn refine_equals_the_plain_definition(
            seed in proptest::prelude::any::<u64>(),
            order in 1usize..=12,
            precision in 5u32..=15,
            ridge in 0usize..4,
        ) {
            let mut state = seed | 1;
            let mut next = move || {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 11) as f64 / (1u64 << 53) as f64
            };
            // A few random sines and some noise: from very predictable to not at all.
            let tones: Vec<(f64, f64)> = (0..3).map(|_| (next() * 0.3, next() * 3000.0)).collect();
            let noise = [0.0, 0.01, 1.0, 100.0][ridge];
            let x: Vec<f64> = (0..1024)
                .map(|t| {
                    let t = f64::from(t);
                    tones.iter().map(|&(f, a)| (t * f).sin() * a).sum::<f64>() + (next() - 0.5) * noise
                })
                .collect();
            let mut autoc = [0.0; MAX_ORDER + 1];
            autocorrelation(&x, &mut autoc[..=order]);
            autoc[0] *= 1.0 + 1e-12;
            let (mut c, mut e) = ([[0.0; MAX_ORDER]; MAX_ORDER], [0.0; MAX_ORDER]);
            let orders = levinson(&autoc, order, &mut c, &mut e);
            for m in 1..=orders {
                let coefs = &c[m - 1][..m];
                let mut q = vec![0i32; m];
                let Some(shift) = quantize(coefs, precision, &mut q) else { continue };
                let mut slow = q.clone();
                refine(coefs, &mut q, shift, precision, &autoc, 8);
                refine_simple(coefs, &mut slow, shift, precision, &autoc, 8);
                proptest::prop_assert_eq!(&q, &slow);
            }
        }
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
