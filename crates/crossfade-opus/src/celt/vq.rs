//! The pyramid vector quantiser's decoding half (`celt/vq.c`): pulses to a unit vector, the
//! spreading rotation, renormalisation.

use super::cwrs::{MAX_PULSES, decode_pulses};
use super::ec::Ec;
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
    if len >= 2 * stride + 1 {
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
        if stride2 != 0 {
            exp_rotation1(block, stride2, s, c);
        }
        exp_rotation1(block, 1, c, s);
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
