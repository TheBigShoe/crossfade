//! Linear prediction for the pitch-based concealment (`celt/celt_lpc.c` and `celt/pitch.c`,
//! float build): autocorrelation, Levinson-Durbin, FIR and IIR filters, and the pitch search.

/// `LPC_ORDER`.
pub(super) const LPC_ORDER: usize = 24;

/// `_celt_lpc`: LPC coefficients from `ac[0..=p]` into `lpc[0..p]`.
pub(super) fn lpc(lpc: &mut [f32], ac: &[f32]) {
    let p = lpc.len();
    lpc.fill(0.0);
    let mut error = ac[0];
    if ac[0] == 0.0 {
        return;
    }
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
        if error < 0.001 * ac[0] {
            break;
        }
    }
}

/// `celt_fir` in place: `x[i] += sum num[j] * mem[j]`, `mem` the past inputs, newest first.
pub(super) fn fir(x: &mut [f32], num: &[f32], mem: &mut [f32]) {
    let ord = num.len();
    for v in x.iter_mut() {
        let mut sum = *v;
        for j in 0..ord {
            sum += num[j] * mem[j];
        }
        mem.copy_within(0..ord - 1, 1);
        mem[0] = *v;
        *v = sum;
    }
}

/// `celt_iir` in place: `x[i] -= sum den[j] * mem[j]`, `mem` the past outputs, newest first.
pub(super) fn iir(x: &mut [f32], den: &[f32], mem: &mut [f32]) {
    let ord = den.len();
    for v in x.iter_mut() {
        let mut sum = *v;
        for j in 0..ord {
            sum -= den[j] * mem[j];
        }
        mem.copy_within(0..ord - 1, 1);
        mem[0] = sum;
        *v = sum;
    }
}

/// `_celt_autocorr` with `lag + 1` outputs; `window` (of `overlap` taps) tapers both ends.
pub(super) fn autocorr(x: &[f32], ac: &mut [f32], window: Option<&[f32]>) {
    let n = x.len();
    let mut xx = x.to_vec();
    if let Some(w) = window {
        for (i, &wi) in w.iter().enumerate().take(n / 2) {
            xx[i] = x[i] * wi;
            xx[n - i - 1] = x[n - i - 1] * wi;
        }
    }
    for (lag, a) in ac.iter_mut().enumerate().rev() {
        let mut d = 0.0f32;
        for i in lag..n {
            d += xx[i] * xx[i - lag];
        }
        *a = d;
    }
    ac[0] += 10.0;
}

/// `pitch_downsample`: the two channels' sum, halved in rate, whitened by a 4th-order LPC.
pub(super) fn pitch_downsample(x: [&[f32]; 2], c: usize, x_lp: &mut [f32]) {
    let half = x_lp.len();
    for (ch, xc) in x.iter().enumerate().take(c) {
        for i in 1..half {
            let v = 0.5 * (0.5 * (xc[2 * i - 1] + xc[2 * i + 1]) + xc[2 * i]);
            if ch == 0 {
                x_lp[i] = v;
            } else {
                x_lp[i] += v;
            }
        }
        let v = 0.5 * (0.5 * xc[1] + xc[0]);
        if ch == 0 {
            x_lp[0] = v;
        } else {
            x_lp[0] += v;
        }
    }
    let mut ac = [0.0f32; 5];
    autocorr(x_lp, &mut ac, None);
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
    let mut mem = [0.0f32; 4];
    fir(x_lp, &lpc4, &mut mem);
    let mut mem = [0.0f32; 1];
    fir(x_lp, &[0.8], &mut mem);
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
            let num = xc * xc;
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

/// `pitch_search`: the lag (in `x_lp`'s half-rate samples, times 2) at which `y` best
/// matches `x_lp`, over `max_pitch` lags.
pub(super) fn pitch_search(x_lp: &[f32], y: &[f32], len: usize, max_pitch: usize) -> usize {
    let lag = len + max_pitch;
    let x4: Vec<f32> = (0..len >> 2).map(|j| x_lp[2 * j]).collect();
    let y4: Vec<f32> = (0..lag >> 2).map(|j| y[2 * j]).collect();
    let mut xcorr = vec![0.0f32; max_pitch >> 1];
    for i in 0..max_pitch >> 2 {
        let mut sum = 0.0f32;
        for j in 0..len >> 2 {
            sum += x4[j] * y4[i + j];
        }
        xcorr[i] = sum.max(-1.0);
    }
    let mut best = [0usize; 2];
    find_best_pitch(&xcorr[..max_pitch >> 2], &y4, len >> 2, &mut best);
    for i in 0..max_pitch >> 1 {
        xcorr[i] = 0.0;
        if (i as isize - 2 * best[0] as isize).abs() > 2
            && (i as isize - 2 * best[1] as isize).abs() > 2
        {
            continue;
        }
        let mut sum = 0.0f32;
        for j in 0..len >> 1 {
            sum += x_lp[j] * y[i + j];
        }
        xcorr[i] = sum.max(-1.0);
    }
    find_best_pitch(&xcorr, y, len >> 1, &mut best);
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

/// `compute_pitch_gain`.
fn pitch_gain(xy: f32, xx: f32, yy: f32) -> f32 {
    xy / (1.0 + xx * yy).sqrt()
}

/// `remove_doubling`: `t0` (a period found in the half-rate `x`, whose history is
/// `max_period / 2` samples before the `n / 2` of the frame) may be a multiple of the true
/// period; looks for a strong correlation at its submultiples, leaning to the last frame's
/// period, and returns the pitch gain with `t0` corrected.
#[allow(clippy::needless_range_loop)] // index loops as the reference writes them
pub(super) fn remove_doubling(
    x: &[f32],
    max_period: usize,
    min_period: usize,
    n: usize,
    t0: &mut usize,
    prev_period: usize,
    prev_gain: f32,
) -> f32 {
    const SECOND_CHECK: [usize; 16] = [0, 0, 3, 2, 3, 2, 5, 2, 3, 2, 3, 2, 5, 2, 3, 2];
    let min_period0 = min_period;
    let max_period = max_period / 2;
    let min_period = min_period / 2;
    let prev_period = (prev_period / 2) as isize;
    let n = n / 2;
    let base = max_period;
    let at = |i: isize| x[(base as isize + i) as usize];
    let start = (*t0 / 2).min(max_period - 1);
    let mut t = start;
    let inner = |lag: usize| -> f32 {
        let mut sum = 0.0f32;
        for i in 0..n {
            sum += x[base + i] * x[base + i - lag];
        }
        sum
    };
    let xx = inner(0);
    let mut xy = inner(start);
    let mut yy_lookup = vec![0.0f32; max_period + 1];
    yy_lookup[0] = xx;
    let mut yy = xx;
    for i in 1..=max_period {
        let (a, b) = (at(-(i as isize)), at(n as isize - i as isize));
        yy = yy + a * a - b * b;
        yy_lookup[i] = yy.max(0.0);
    }
    yy = yy_lookup[start];
    let mut best_xy = xy;
    let mut best_yy = yy;
    let g0 = pitch_gain(xy, xx, yy);
    let mut g = g0;
    // Any pitch at T / k.
    for k in 2..=15usize {
        let t1 = (2 * start + k) / (2 * k);
        if t1 < min_period {
            break;
        }
        // Another strong correlation is wanted at t1b.
        let t1b = if k == 2 {
            if t1 + start > max_period {
                start
            } else {
                start + t1
            }
        } else {
            (2 * SECOND_CHECK[k] * start + k) / (2 * k)
        };
        xy = 0.5 * (inner(t1) + inner(t1b));
        yy = 0.5 * (yy_lookup[t1] + yy_lookup[t1b]);
        let g1 = pitch_gain(xy, xx, yy);
        let gap = (t1 as isize - prev_period).abs();
        let cont = if gap <= 1 {
            prev_gain
        } else if gap <= 2 && 5 * k * k < start {
            0.5 * prev_gain
        } else {
            0.0
        };
        // Against very short periods, which short-term correlation fakes.
        let thresh = if t1 < 3 * min_period {
            (0.85 * g0 - cont).max(0.4)
        } else if t1 < 2 * min_period {
            (0.9 * g0 - cont).max(0.5)
        } else {
            (0.7 * g0 - cont).max(0.3)
        };
        if g1 > thresh {
            best_xy = xy;
            best_yy = yy;
            t = t1;
            g = g1;
        }
    }
    let best_xy = best_xy.max(0.0);
    let pg = if best_yy <= best_xy {
        1.0
    } else {
        best_xy / (best_yy + 1.0)
    };
    let xcorr = [
        inner(t.saturating_sub(1)),
        inner(t),
        inner((t + 1).min(max_period)),
    ];
    let offset: isize = if (xcorr[2] - xcorr[0]) > 0.7 * (xcorr[1] - xcorr[0]) {
        1
    } else if (xcorr[0] - xcorr[2]) > 0.7 * (xcorr[1] - xcorr[2]) {
        -1
    } else {
        0
    };
    *t0 = ((2 * t as isize + offset).max(min_period0 as isize)) as usize;
    pg.min(g)
}

#[cfg(test)]
#[path = "lpc_tests.rs"]
mod tests;
