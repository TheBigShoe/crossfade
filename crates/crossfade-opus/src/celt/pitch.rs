//! The pre-filter's pitch analysis (`celt/pitch.c` and `celt/celt_lpc.c` of libopus 1.5.2,
//! float build): the whitened half-rate signal, the two-stage correlation search, and the
//! search for a period's submultiples. The decoder's concealment keeps its own (older) copy
//! in `lpc.rs`, which its output depends on.
#![allow(clippy::needless_range_loop)] // index loops as the reference writes them

/// `_celt_autocorr` without a window: `ac[0..=lag]` of `x`.
fn autocorr(x: &[f32], ac: &mut [f32]) {
    let n = x.len();
    let lag = ac.len() - 1;
    let fast_n = n - lag;
    for (k, a) in ac.iter_mut().enumerate() {
        let mut sum = 0.0f32;
        for j in 0..fast_n {
            sum += x[j] * x[j + k];
        }
        let mut d = 0.0f32;
        for i in k + fast_n..n {
            d += x[i] * x[i - k];
        }
        *a = sum + d;
    }
}

/// `_celt_lpc`: LPC coefficients from `ac[0..=p]` into `lpc[0..p]` (Levinson-Durbin).
fn lpc(lpc: &mut [f32], ac: &[f32]) {
    let p = lpc.len();
    lpc.fill(0.0);
    let mut error = ac[0];
    if ac[0] > 1e-10 {
        for i in 0..p {
            let mut rr = 0.0f32;
            for j in 0..i {
                rr += lpc[j] * ac[i - j];
            }
            rr += ac[i + 1];
            let r = -(rr / error);
            lpc[i] = r;
            for j in 0..(i + 1) >> 1 {
                let tmp1 = lpc[j];
                let tmp2 = lpc[i - 1 - j];
                lpc[j] = tmp1 + r * tmp2;
                lpc[i - 1 - j] = tmp2 + r * tmp1;
            }
            error -= (r * r) * error;
            // Stop at 30 dB of prediction gain.
            if error <= 0.001 * ac[0] {
                break;
            }
        }
    }
}

/// `celt_fir5` in place.
fn fir5(x: &mut [f32], num: &[f32; 5]) {
    let mut mem = [0.0f32; 5];
    for v in x.iter_mut() {
        let mut sum = *v;
        for k in 0..5 {
            sum += num[k] * mem[k];
        }
        mem.copy_within(0..4, 1);
        mem[0] = *v;
        *v = sum;
    }
}

/// `pitch_downsample`: the channels' sum at half the rate (`x_lp.len()` samples from twice
/// as many per channel), whitened by a 4th-order LPC with a zero added.
pub(super) fn pitch_downsample(x: [&[f32]; 2], c: usize, x_lp: &mut [f32]) {
    let half = x_lp.len();
    for (ch, xc) in x.iter().enumerate().take(c) {
        for i in 1..half {
            let v = 0.25 * xc[2 * i - 1] + 0.25 * xc[2 * i + 1] + 0.5 * xc[2 * i];
            if ch == 0 {
                x_lp[i] = v;
            } else {
                x_lp[i] += v;
            }
        }
        let v = 0.25 * xc[1] + 0.5 * xc[0];
        if ch == 0 {
            x_lp[0] = v;
        } else {
            x_lp[0] += v;
        }
    }
    let mut ac = [0.0f32; 5];
    autocorr(x_lp, &mut ac);
    // A noise floor at -40 dB, and lag windowing.
    ac[0] *= 1.0001;
    for (i, a) in ac.iter_mut().enumerate().skip(1) {
        *a -= *a * (0.008 * i as f32) * (0.008 * i as f32);
    }
    let mut lpc4 = [0.0f32; 4];
    lpc(&mut lpc4, &ac);
    let mut tmp = 1.0f32;
    for l in lpc4.iter_mut() {
        tmp *= 0.9;
        *l *= tmp;
    }
    let c1 = 0.8f32;
    let lpc2 = [
        lpc4[0] + 0.8,
        lpc4[1] + c1 * lpc4[0],
        lpc4[2] + c1 * lpc4[1],
        lpc4[3] + c1 * lpc4[2],
        c1 * lpc4[3],
    ];
    fir5(x_lp, &lpc2);
}

fn inner(a: &[f32], b: &[f32]) -> f32 {
    let mut s = 0.0f32;
    for (&x, &y) in a.iter().zip(b) {
        s += x * y;
    }
    s
}

/// `find_best_pitch`: the two lags of largest normalised correlation.
fn find_best_pitch(xcorr: &[f32], y: &[f32], len: usize, best: &mut [usize; 2]) {
    let mut syy = 1.0f32;
    let mut best_num = [-1.0f32; 2];
    let mut best_den = [0.0f32; 2];
    *best = [0, 1];
    for &v in &y[..len] {
        syy += v * v;
    }
    for (i, &xc) in xcorr.iter().enumerate() {
        if xc > 0.0 {
            // Scaled so that squaring neither underflows nor overflows.
            let xc16 = xc * 1e-12;
            let num = xc16 * xc16;
            if num * best_den[1] > best_num[1] * syy {
                if num * best_den[0] > best_num[0] * syy {
                    best_num[1] = best_num[0];
                    best_den[1] = best_den[0];
                    best[1] = best[0];
                    best_num[0] = num;
                    best_den[0] = syy;
                    best[0] = i;
                } else {
                    best_num[1] = num;
                    best_den[1] = syy;
                    best[1] = i;
                }
            }
        }
        syy += y[i + len] * y[i + len] - y[i] * y[i];
        syy = syy.max(1.0);
    }
}

/// `pitch_search`: the lag, in `y`'s samples, at which `y` best matches `x_lp` (`len`
/// samples at twice `x_lp`'s rate), over `max_pitch` lags: a search at a quarter of the rate,
/// refined at half and by interpolation.
pub(super) fn pitch_search(x_lp: &[f32], y: &[f32], len: usize, max_pitch: usize) -> usize {
    let lag = len + max_pitch;
    let mut x4 = [0.0f32; 240];
    let mut y4 = [0.0f32; 500];
    let x4 = &mut x4[..len >> 2];
    let y4 = &mut y4[..lag >> 2];
    for (j, v) in x4.iter_mut().enumerate() {
        *v = x_lp[2 * j];
    }
    for (j, v) in y4.iter_mut().enumerate() {
        *v = y[2 * j];
    }
    let mut xcorr = [0.0f32; 512];
    let xcorr = &mut xcorr[..max_pitch >> 1];
    for i in 0..max_pitch >> 2 {
        xcorr[i] = inner(x4, &y4[i..]);
    }
    let mut best = [0usize; 2];
    find_best_pitch(&xcorr[..max_pitch >> 2], y4, len >> 2, &mut best);
    for i in 0..max_pitch >> 1 {
        xcorr[i] = 0.0;
        if i.abs_diff(2 * best[0]) > 2 && i.abs_diff(2 * best[1]) > 2 {
            continue;
        }
        xcorr[i] = inner(&x_lp[..len >> 1], &y[i..]).max(-1.0);
    }
    find_best_pitch(xcorr, y, len >> 1, &mut best);
    // Pseudo-interpolation.
    let offset: isize = if best[0] > 0 && best[0] < (max_pitch >> 1) - 1 {
        let a = xcorr[best[0] - 1];
        let b = xcorr[best[0]];
        let c = xcorr[best[0] + 1];
        if (c - a) > 0.7 * (b - a) {
            1
        } else if (a - c) > 0.7 * (b - c) {
            -1
        } else {
            0
        }
    } else {
        0
    };
    (2 * best[0] as isize - offset).max(0) as usize
}

/// `compute_pitch_gain` (float build).
fn pitch_gain(xy: f32, xx: f32, yy: f32) -> f32 {
    xy / (1.0 + xx * yy).sqrt()
}

/// Correlations from libopus's `second_check`.
const SECOND_CHECK: [usize; 16] = [0, 0, 3, 2, 3, 2, 5, 2, 3, 2, 3, 2, 5, 2, 3, 2];

/// `remove_doubling`: looks for a submultiple of the period `t0` (in `x`'s full-rate samples;
/// `x` is the half-rate signal, `maxperiod / 2` of history then `n / 2`) that correlates as
/// well, so that the pre-filter does not lock onto an octave below. Returns the gain; `t0`
/// becomes the period chosen, at least `minperiod`.
pub(super) fn remove_doubling(
    x: &[f32],
    maxperiod: usize,
    minperiod: usize,
    n: usize,
    t0: &mut usize,
    prev_period: usize,
    prev_gain: f32,
) -> f32 {
    let minperiod0 = minperiod;
    let maxperiod = maxperiod / 2;
    let minperiod = minperiod / 2;
    let prev_period = prev_period / 2;
    let n = n / 2;
    // x[at + k] is the reference's x[k].
    let at = maxperiod;
    let mut t0h = (*t0 / 2).min(maxperiod - 1);
    let mut yy_lookup = [0.0f32; 513];
    let xs = &x[at..at + n];
    let xx = inner(xs, xs);
    let mut xy = inner(xs, &x[at - t0h..]);
    yy_lookup[0] = xx;
    let mut yy = xx;
    for i in 1..=maxperiod {
        yy = yy + x[at - i] * x[at - i] - x[at + n - i] * x[at + n - i];
        yy_lookup[i] = yy.max(0.0);
    }
    yy = yy_lookup[t0h];
    let mut best_xy = xy;
    let mut best_yy = yy;
    let g0 = pitch_gain(xy, xx, yy);
    let mut g = g0;
    let mut t = t0h;
    // A pitch at T/k?
    for k in 2..=15usize {
        let t1 = (2 * t0h + k) / (2 * k);
        if t1 < minperiod {
            break;
        }
        // Another strong correlation at t1b.
        let t1b = if k == 2 {
            if t1 + t0h > maxperiod { t0h } else { t0h + t1 }
        } else {
            (2 * SECOND_CHECK[k] * t0h + k) / (2 * k)
        };
        let xy1 = inner(xs, &x[at - t1..]);
        let xy2 = inner(xs, &x[at - t1b..]);
        xy = 0.5 * (xy1 + xy2);
        yy = 0.5 * (yy_lookup[t1] + yy_lookup[t1b]);
        let g1 = pitch_gain(xy, xx, yy);
        let cont = if t1.abs_diff(prev_period) <= 1 {
            prev_gain
        } else if t1.abs_diff(prev_period) <= 2 && 5 * k * k < t0h {
            0.5 * prev_gain
        } else {
            0.0
        };
        let mut thresh = 0.3f32.max(0.7 * g0 - cont);
        // Against very short periods: short-term correlation gives false positives.
        if t1 < 3 * minperiod {
            thresh = 0.4f32.max(0.85 * g0 - cont);
        } else if t1 < 2 * minperiod {
            thresh = 0.5f32.max(0.9 * g0 - cont);
        }
        if g1 > thresh {
            best_xy = xy;
            best_yy = yy;
            t = t1;
            g = g1;
        }
    }
    let best_xy = best_xy.max(0.0);
    let mut pg = if best_yy <= best_xy {
        1.0
    } else {
        best_xy / (best_yy + 1.0)
    };
    let mut xcorr = [0.0f32; 3];
    for (k, v) in xcorr.iter_mut().enumerate() {
        // Lags t - 1, t, t + 1; t is at least 1 here when minperiod is.
        *v = inner(xs, &x[at + 1 - (t + k)..]);
    }
    let offset: isize = if (xcorr[2] - xcorr[0]) > 0.7 * (xcorr[1] - xcorr[0]) {
        1
    } else if (xcorr[0] - xcorr[2]) > 0.7 * (xcorr[1] - xcorr[2]) {
        -1
    } else {
        0
    };
    if pg > g {
        pg = g;
    }
    t0h = (2 * t as isize + offset).max(0) as usize;
    *t0 = t0h.max(minperiod0);
    pg
}

#[cfg(test)]
#[path = "pitch_tests.rs"]
mod tests;
