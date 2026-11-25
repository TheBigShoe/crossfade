//! The pyramid vector quantiser's decoding half (`celt/vq.c`): pulses to a unit vector, the
//! spreading rotation, renormalisation.

use super::cwrs::{MAX_PULSES, decode_pulses, encode_pulses};
use super::ec::Ec;
use super::maths::cos_norm;
use crate::range_enc::RangeEncoder;

pub(super) const SPREAD_NONE: i32 = 0;
pub(super) const SPREAD_AGGRESSIVE: i32 = 3;

/// `exp_rotation1`: Givens rotations of pairs `stride` apart, forwards then backwards.
fn exp_rotation1(x: &mut [f32], stride: usize, c: f32, s: f32) {
    let len = x.len();
    if len <= stride {
        return;
    }
    for i in 0..len - stride {
        let x1 = x[i];
        let x2 = x[i + stride];
        x[i + stride] = c * x2 + s * x1;
        x[i] = c * x1 - s * x2;
    }
    if len > 2 * stride {
        for i in (0..len - 2 * stride).rev() {
            let x1 = x[i];
            let x2 = x[i + stride];
            x[i + stride] = c * x2 + s * x1;
            x[i] = c * x1 - s * x2;
        }
    }
}

/// `exp_rotation` with `dir < 0` (the decoder's direction).
fn exp_rotation(x: &mut [f32], stride: usize, k: usize, spread: i32) {
    exp_rotation_dir(x, stride, k, spread, false);
}

/// `exp_rotation`: `forward` is the encoder's direction (`dir > 0`), undone by the other.
fn exp_rotation_dir(x: &mut [f32], stride: usize, k: usize, spread: i32, forward: bool) {
    const SPREAD_FACTOR: [usize; 3] = [15, 10, 5];
    let len = x.len();
    if 2 * k >= len || spread == SPREAD_NONE || !(1..=3).contains(&spread) || stride == 0 {
        return;
    }
    let factor = SPREAD_FACTOR[spread as usize - 1];
    let gain = len as f32 / (len + factor * k) as f32;
    let theta = 0.5 * (gain * gain);
    let c = cos_norm(theta);
    let s = cos_norm(1.0 - theta);
    let mut stride2 = 0;
    if len >= 8 * stride {
        stride2 = 1;
        while (stride2 * stride2 + stride2) * stride + (stride >> 2) < len {
            stride2 += 1;
        }
    }
    let sub = len / stride;
    if sub == 0 {
        return;
    }
    for block in x.chunks_exact_mut(sub).take(stride) {
        if forward {
            exp_rotation1(block, 1, c, -s);
            if stride2 != 0 {
                exp_rotation1(block, stride2, s, -c);
            }
        } else {
            if stride2 != 0 {
                exp_rotation1(block, stride2, s, c);
            }
            exp_rotation1(block, 1, c, s);
        }
    }
}

/// `op_pvq_search`: the `k`-pulse integer vector `iy` nearest `x` in direction (signs as
/// `x`'s); `x` is left with its absolute values. Returns the pulses' energy, exactly.
fn pvq_search(x: &mut [f32], iy: &mut [i32], k: usize) -> f32 {
    let n = x.len();
    let mut y = [0.0f32; 176];
    let mut signx = [false; 176];
    for j in 0..n {
        signx[j] = x[j] < 0.0;
        x[j] = x[j].abs();
        iy[j] = 0;
    }
    let mut xy = 0.0f32;
    let mut yy = 0.0f32;
    let mut pulses_left = k as i32;
    if k > n >> 1 {
        let mut sum = 0.0f32;
        for &v in x.iter() {
            sum += v;
        }
        // Also catches infinities and NaNs: 64 stands for infinity here.
        if !(sum > 1e-15 && sum < 64.0) {
            x[0] = 1.0;
            x[1..].fill(0.0);
            sum = 1.0;
        }
        // K + e with e < 1 cannot give more than K pulses.
        let rcp = (k as f32 + 0.8) * (1.0 / sum);
        for j in 0..n {
            iy[j] = libm::floorf(rcp * x[j]) as i32;
            y[j] = iy[j] as f32;
            yy += y[j] * y[j];
            xy += x[j] * y[j];
            y[j] *= 2.0;
            pulses_left -= iy[j];
        }
        if pulses_left < 0 {
            // Not reachable with finite input; start over rather than code the wrong count.
            iy[..n].fill(0);
            y[..n].fill(0.0);
            xy = 0.0;
            yy = 0.0;
            pulses_left = k as i32;
        }
    }
    if pulses_left > n as i32 + 3 {
        let tmp = pulses_left as f32;
        yy += tmp * tmp;
        yy += tmp * y[0];
        iy[0] += pulses_left;
        pulses_left = 0;
    }
    for _ in 0..pulses_left {
        let mut best_id = 0;
        yy += 1.0;
        let rxy = xy + x[0];
        let mut best_den = yy + y[0];
        let mut best_num = rxy * rxy;
        for j in 1..n {
            let rxy = xy + x[j];
            let ryy = yy + y[j];
            let rxy = rxy * rxy;
            if best_den * rxy > ryy * best_num {
                best_den = ryy;
                best_num = rxy;
                best_id = j;
            }
        }
        xy += x[best_id];
        yy += y[best_id];
        y[best_id] += 2.0;
        iy[best_id] += 1;
    }
    for j in 0..n {
        if signx[j] {
            iy[j] = -iy[j];
        }
    }
    yy
}

/// `alg_quant` with resynthesis: codes the shape `x` (2 to 176 values) with `k` pulses, leaves
/// in `x` what [`alg_unquant`] will decode, and returns the collapse mask of its `b` blocks.
pub(super) fn alg_quant(
    x: &mut [f32],
    k: usize,
    spread: i32,
    b: usize,
    enc: &mut RangeEncoder,
    gain: f32,
) -> u32 {
    let n = x.len().min(176);
    let x = &mut x[..n];
    let k = k.min(MAX_PULSES);
    let mut iy = [0i32; 176];
    let iy = &mut iy[..n];
    exp_rotation_dir(x, b, k, spread, true);
    let yy = pvq_search(x, iy, k);
    encode_pulses(iy, k, enc);
    let g = (1.0 / yy.sqrt()) * gain;
    for (x, &v) in x.iter_mut().zip(iy.iter()) {
        *x = g * v as f32;
    }
    exp_rotation(x, b, k, spread);
    extract_collapse_mask(iy, b)
}

/// `fast_atan2f` of `mathops.h`: a rational approximation, in float.
fn fast_atan2f(y: f32, x: f32) -> f32 {
    const CA: f32 = 0.431_579_74;
    const CB: f32 = 0.678_484_03;
    const CC: f32 = 0.085_955_42;
    const CE: f32 = std::f32::consts::PI / 2.0;
    let x2 = x * x;
    let y2 = y * y;
    if x2 + y2 < 1e-18 {
        return 0.0;
    }
    let cy = if y < 0.0 { -CE } else { CE };
    if x2 < y2 {
        let den = (y2 + CB * x2) * (y2 + CC * x2);
        -x * y * (y2 + CA * x2) / den + cy
    } else {
        let den = (x2 + CB * y2) * (x2 + CC * y2);
        let cxy = if x * y < 0.0 { -CE } else { CE };
        x * y * (x2 + CA * y2) / den + cy - cxy
    }
}

/// `stereo_itheta`: the angle (0 to 16384 for 0 to pi/2) between mid and side, of the two
/// halves `x` and `y` of a split (`stereo` false) or of the mid and side of two channels.
pub(super) fn stereo_itheta(x: &[f32], y: &[f32], stereo: bool) -> i32 {
    let mut emid = 1e-15f32;
    let mut eside = 1e-15f32;
    if stereo {
        for (&a, &b) in x.iter().zip(y) {
            let m = a + b;
            let s = a - b;
            emid += m * m;
            eside += s * s;
        }
    } else {
        let mut ex = 0.0f32;
        let mut ey = 0.0f32;
        for &a in x {
            ex += a * a;
        }
        for &b in y {
            ey += b * b;
        }
        emid += ex;
        eside += ey;
    }
    let mid = emid.sqrt();
    let side = eside.sqrt();
    libm::floorf(0.5 + 16384.0 * 0.63662 * fast_atan2f(side, mid)) as i32
}

/// `extract_collapse_mask`: which of the `b` interleaved blocks got a pulse.
fn extract_collapse_mask(iy: &[i32], b: usize) -> u32 {
    if b <= 1 {
        return 1;
    }
    let n0 = iy.len() / b;
    let mut mask = 0;
    for (i, block) in iy.chunks_exact(n0.max(1)).take(b).enumerate() {
        if block.iter().any(|&v| v != 0) {
            mask |= 1 << i;
        }
    }
    mask
}

/// `alg_unquant`: decodes `k` pulses into the unit vector `x` scaled by `gain`, and returns
/// the collapse mask of its `b` blocks.
pub(super) fn alg_unquant(
    x: &mut [f32],
    k: usize,
    spread: i32,
    b: usize,
    ec: &mut Ec<'_, '_>,
    gain: f32,
) -> u32 {
    let n = x.len();
    let mut iy = [0i32; 176];
    let iy = &mut iy[..n.min(176)];
    decode_pulses(iy, k.min(MAX_PULSES), ec);
    let mut ryy = 0.0f32;
    for &v in iy.iter() {
        ryy += (v as f32) * (v as f32);
    }
    let g = (1.0 / ryy.sqrt()) * gain;
    for (x, &v) in x.iter_mut().zip(iy.iter()) {
        *x = g * v as f32;
    }
    exp_rotation(x, b, k, spread);
    extract_collapse_mask(iy, b)
}

/// `renormalise_vector`: scales `x` to norm `gain`.
pub(super) fn renormalise_vector(x: &mut [f32], gain: f32) {
    let mut e = 1e-15f32;
    for &v in x.iter() {
        e += v * v;
    }
    let g = (1.0 / e.sqrt()) * gain;
    for v in x.iter_mut() {
        *v *= g;
    }
}

#[cfg(test)]
#[path = "vq_tests.rs"]
mod tests;
