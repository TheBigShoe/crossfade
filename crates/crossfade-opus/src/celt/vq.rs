//! The pyramid vector quantiser (`celt/vq.c`): the search for the pulses nearest a shape and
//! their coding, pulses back to a unit vector, the spreading rotation, renormalisation.

use super::cwrs::{MAX_PULSES, decode_pulses, encode_pulses};
use super::ec::{Ec, Enc};
use super::maths::cos_norm;

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

/// The rotation `exp_rotation` applies for `k` pulses over `len` bins: its cosine, sine and
/// second stride, or `None` when it applies none.
fn rotation(len: usize, stride: usize, k: usize, spread: i32) -> Option<(f32, f32, usize)> {
    const SPREAD_FACTOR: [usize; 3] = [15, 10, 5];
    if 2 * k >= len || spread == SPREAD_NONE || !(1..=3).contains(&spread) || stride == 0 {
        return None;
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
    (len / stride > 0).then_some((c, s, stride2))
}

/// `exp_rotation` with `dir < 0` (the decoder's direction).
fn exp_rotation(x: &mut [f32], stride: usize, k: usize, spread: i32) {
    let Some((c, s, stride2)) = rotation(x.len(), stride, k, spread) else {
        return;
    };
    let sub = x.len() / stride;
    for block in x.chunks_exact_mut(sub).take(stride) {
        if stride2 != 0 {
            exp_rotation1(block, stride2, s, c);
        }
        exp_rotation1(block, 1, c, s);
    }
}

/// `exp_rotation` with `dir > 0`: what the encoder applies before the search, undone by
/// [`exp_rotation`].
fn exp_rotation_forward(x: &mut [f32], stride: usize, k: usize, spread: i32) {
    let Some((c, s, stride2)) = rotation(x.len(), stride, k, spread) else {
        return;
    };
    let sub = x.len() / stride;
    for block in x.chunks_exact_mut(sub).take(stride) {
        exp_rotation1(block, 1, c, -s);
        if stride2 != 0 {
            exp_rotation1(block, stride2, s, -c);
        }
    }
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

/// `normalise_residual`: the pulses `iy` as a vector of norm `gain`.
fn normalise_residual(iy: &[i32], x: &mut [f32], gain: f32) {
    let mut ryy = 0.0f32;
    for &v in iy.iter() {
        ryy += (v as f32) * (v as f32);
    }
    let g = (1.0 / ryy.sqrt()) * gain;
    for (x, &v) in x.iter_mut().zip(iy.iter()) {
        *x = g * v as f32;
    }
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
    normalise_residual(iy, x, gain);
    exp_rotation(x, b, k, spread);
    extract_collapse_mask(iy, b)
}

/// `op_pvq_search`: the `k` signed pulses over `x.len()` bins nearest the direction of `x`
/// (greedy, after a projection onto the pyramid when there are many pulses). `x` is left as
/// its absolute values.
fn op_pvq_search(x: &mut [f32], iy: &mut [i32], k: usize) {
    let n = x.len();
    let mut y = [0.0f32; 176];
    let mut neg = [false; 176];
    for j in 0..n {
        neg[j] = x[j] < 0.0;
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
        // Silence, infinities and NaNs: a pulse train at bin 0 instead (64 stands for
        // infinity here).
        if !(sum > 1e-15 && sum < 64.0) {
            x[0] = 1.0;
            x[1..].fill(0.0);
            sum = 1.0;
        }
        // K + e with e < 1 cannot give more than K pulses.
        let rcp = (k as f32 + 0.8) * (1.0 / sum);
        for j in 0..n {
            iy[j] = (rcp * x[j]).floor() as i32;
            y[j] = iy[j] as f32;
            yy += y[j] * y[j];
            xy += x[j] * y[j];
            y[j] *= 2.0;
            pulses_left -= iy[j];
        }
    }
    let mut pulses_left = pulses_left.max(0);
    if pulses_left > n as i32 + 3 {
        // Should not happen; all the rest in the first bin if it does.
        iy[0] += pulses_left;
        pulses_left = 0;
    }
    for _ in 0..pulses_left {
        // The new pulse's square is added whichever bin gets it.
        yy += 1.0;
        let mut best_id = 0;
        let rxy = xy + x[0];
        let mut best_den = yy + y[0];
        let mut best_num = rxy * rxy;
        for j in 1..n {
            let rxy = xy + x[j];
            let ryy = yy + y[j];
            let rxy = rxy * rxy;
            // rxy / sqrt(ryy) is maximised, without the division.
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
        if neg[j] {
            iy[j] = -iy[j];
        }
    }
}

/// `alg_quant`: codes the shape `x` (2 to 176 bins) with `k` pulses and leaves in `x` what
/// the decoder will rebuild, scaled by `gain`; returns the collapse mask of its `b` blocks.
#[allow(dead_code)] // ENCODER-WIP
pub(super) fn alg_quant(
    x: &mut [f32],
    k: usize,
    spread: i32,
    b: usize,
    enc: &mut Enc<'_>,
    gain: f32,
) -> u32 {
    let n = x.len().min(176);
    let x = &mut x[..n];
    let k = k.clamp(1, MAX_PULSES);
    let mut iy = [0i32; 176];
    let iy = &mut iy[..n];
    exp_rotation_forward(x, b, k, spread);
    op_pvq_search(x, iy, k);
    encode_pulses(iy, k, enc);
    normalise_residual(iy, x, gain);
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
