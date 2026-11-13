//! The hybrid filterbank's MDCT half: short-block reordering, alias reduction, the IMDCT with
//! its windows and overlap, and frequency inversion (ISO/IEC 11172-3 2.4.3.4.10).
//!
//! The IMDCT algorithms (a 36-point IMDCT from two 9-point DCT-IIIs, a 12-point one from two
//! 3-point ones) and their constants follow minimp3 (public domain, CC0 1.0) by way of
//! nanomp3-core (MIT OR Apache-2.0, Copyright (c) 2025 Robert B. Langer).
#![allow(clippy::excessive_precision)]

use crate::lanes::{F4, Lane};
use crate::sideinfo::{Bands, SHORT_BLOCK, STOP_BLOCK};
use crate::tables::{ANTIALIAS, IMDCT12_TWID3, IMDCT36_TWID9, MDCT_WINDOW};

/// Short blocks come band by band, each band window by window; the IMDCT wants each line's
/// three windows side by side. `xr` starts at the first short line.
pub(crate) fn reorder(xr: &mut [f32], scratch: &mut [f32; 576], bands: &Bands) {
    let short = &bands.width[usize::from(bands.n_long)..bands.count()];
    let mut src = 0;
    let mut dst = 0;
    for &w in short.iter().step_by(3) {
        let w = usize::from(w);
        for i in 0..w {
            scratch[dst] = xr[src + i];
            scratch[dst + 1] = xr[src + w + i];
            scratch[dst + 2] = xr[src + 2 * w + i];
            dst += 3;
        }
        src += 3 * w;
    }
    xr[..dst].copy_from_slice(&scratch[..dst]);
}

/// Alias-reduction butterflies across the first `boundaries` subband boundaries.
pub(crate) fn antialias(xr: &mut [f32; 576], boundaries: usize) {
    let aa = |k: usize, i: usize| F4::load(&ANTIALIAS[k][i..]);
    for b in 0..boundaries {
        let (lo, hi) = xr[18 * b..18 * b + 36].split_at_mut(18);
        for i in [0, 4] {
            let u = F4::load(&hi[i..]);
            let d = F4::load(&lo[14 - i..]).rev();
            (u * aa(0, i) - d * aa(1, i)).store(&mut hi[i..]);
            (u * aa(1, i) + d * aa(0, i)).rev().store(&mut lo[14 - i..]);
        }
    }
}

#[inline(always)]
fn dct3_9<T: Lane>(y: &mut [T; 9]) {
    let c = T::splat;
    let mut s0 = y[0];
    let mut s2 = y[2];
    let mut s4 = y[4];
    let mut s6 = y[6];
    let mut s8 = y[8];
    let mut t0 = s0 + s6 * c(0.5);
    s0 = s0 - s6;
    let mut t4 = (s4 + s2) * c(0.93969262);
    let mut t2 = (s8 + s2) * c(0.76604444);
    s6 = (s4 - s8) * c(0.17364818);
    s4 = s4 + (s8 - s2);

    s2 = s0 - s4 * c(0.5);
    y[4] = s4 + s0;
    s8 = t0 - t2 + s6;
    s0 = t0 - t4 + t2;
    s4 = t0 + t4 - s6;

    let mut s1 = y[1];
    let mut s3 = y[3];
    let mut s5 = y[5];
    let mut s7 = y[7];

    s3 = s3 * c(0.86602540);
    t0 = (s5 + s1) * c(0.98480775);
    t4 = (s5 - s7) * c(0.34202014);
    t2 = (s1 + s7) * c(0.64278761);
    s1 = (s1 - s5 - s7) * c(0.86602540);

    s5 = t0 - s3 - t2;
    s7 = t4 - s3 - t0;
    s3 = t4 + s3 - t2;

    y[0] = s4 - s7;
    y[1] = s2 + s1;
    y[2] = s0 - s3;
    y[3] = s8 + s5;
    y[5] = s8 - s5;
    y[6] = s0 + s3;
    y[7] = s2 - s1;
    y[8] = s4 + s7;
}

/// The 36-point IMDCT of `T`'s lanes' worth of adjacent subbands at once, windowed and
/// overlapped. `overlap` keeps 9 values per subband.
#[inline(always)]
fn imdct36_bands<T: Lane>(xr: &mut [f32], overlap: &mut [f32], window: &[f32; 18]) {
    let c = T::splat;
    let g = |xr: &[f32], o: usize| T::gather(&xr[o..], 18);
    let mut co = [c(0.0); 9];
    let mut si = [c(0.0); 9];
    co[0] = -g(xr, 0);
    si[0] = g(xr, 17);
    for i in 0..4 {
        si[8 - 2 * i] = g(xr, 4 * i + 1) - g(xr, 4 * i + 2);
        co[1 + 2 * i] = g(xr, 4 * i + 1) + g(xr, 4 * i + 2);
        si[7 - 2 * i] = g(xr, 4 * i + 4) - g(xr, 4 * i + 3);
        co[2 + 2 * i] = -(g(xr, 4 * i + 3) + g(xr, 4 * i + 4));
    }
    dct3_9(&mut co);
    dct3_9(&mut si);
    si[1] = -si[1];
    si[3] = -si[3];
    si[5] = -si[5];
    si[7] = -si[7];
    for i in 0..9 {
        let ovl = T::gather(&overlap[i..], 9);
        let sum = co[i] * c(IMDCT36_TWID9[9 + i]) + si[i] * c(IMDCT36_TWID9[i]);
        (co[i] * c(IMDCT36_TWID9[i]) - si[i] * c(IMDCT36_TWID9[9 + i]))
            .scatter(&mut overlap[i..], 9);
        (ovl * c(window[i]) - sum * c(window[9 + i])).scatter(&mut xr[i..], 18);
        (ovl * c(window[9 + i]) + sum * c(window[i])).scatter(&mut xr[17 - i..], 18);
    }
}

fn imdct36(xr: &mut [f32], overlap: &mut [f32], window: &[f32; 18], subbands: usize) {
    let mut j = 0;
    while j + 4 <= subbands {
        imdct36_bands::<F4>(&mut xr[18 * j..], &mut overlap[9 * j..], window);
        j += 4;
    }
    while j < subbands {
        imdct36_bands::<f32>(&mut xr[18 * j..], &mut overlap[9 * j..], window);
        j += 1;
    }
}

fn idct3(x0: f32, x1: f32, x2: f32) -> [f32; 3] {
    let m1 = x1 * 0.86602540;
    let a1 = x0 - x2 * 0.5;
    [a1 + m1, x0 + x2, a1 - m1]
}

fn imdct12(x: &[f32], dst: &mut [f32], overlap: &mut [f32]) {
    let co = idct3(-x[0], x[6] + x[3], x[12] + x[9]);
    let mut si = idct3(x[15], x[12] - x[9], x[6] - x[3]);
    si[1] = -si[1];
    let t = &IMDCT12_TWID3;
    for i in 0..3 {
        let ovl = overlap[i];
        let sum = co[i] * t[3 + i] + si[i] * t[i];
        overlap[i] = co[i] * t[i] - si[i] * t[3 + i];
        dst[i] = ovl * t[2 - i] - sum * t[5 - i];
        dst[5 - i] = ovl * t[5 - i] + sum * t[2 - i];
    }
}

fn imdct_short(xr: &mut [f32], overlap: &mut [f32], subbands: usize) {
    for (x, ov) in xr
        .as_chunks_mut::<18>()
        .0
        .iter_mut()
        .zip(overlap.as_chunks_mut::<9>().0)
        .take(subbands)
    {
        let mut tmp = [0f32; 18];
        tmp.copy_from_slice(x);
        x[..6].copy_from_slice(&ov[..6]);
        let (ov_out, ov_state) = ov.split_at_mut(6);
        imdct12(&tmp, &mut x[6..12], ov_state);
        imdct12(&tmp[1..], &mut x[12..18], ov_state);
        imdct12(&tmp[2..], ov_out, ov_state);
    }
}

/// The IMDCT of a granule's first `subbands` subbands (the rest, and their overlap, are zero):
/// the first `long_subbands` with the normal window, the rest as `block_type` says.
pub(crate) fn imdct(
    xr: &mut [f32; 576],
    overlap: &mut [f32; 288],
    block_type: u8,
    long_subbands: usize,
    subbands: usize,
) {
    let subbands = subbands.min(32);
    let long = long_subbands.min(subbands);
    let (long_x, rest_x) = xr.split_at_mut(18 * long);
    let (long_o, rest_o) = overlap.split_at_mut(9 * long);
    if long > 0 {
        imdct36(long_x, long_o, &MDCT_WINDOW[0], long);
    }
    if block_type == SHORT_BLOCK {
        imdct_short(rest_x, rest_o, subbands - long);
    } else {
        let window = &MDCT_WINDOW[usize::from(block_type == STOP_BLOCK)];
        imdct36(rest_x, rest_o, window, subbands - long);
    }
}

/// Frequency inversion: every odd sample of every odd subband changes sign (of the first
/// `subbands`).
pub(crate) fn invert_odd(xr: &mut [f32; 576], subbands: usize) {
    for band in xr.as_chunks_mut::<36>().0.iter_mut().take(subbands / 2) {
        for x in band[18..].iter_mut().skip(1).step_by(2) {
            *x = -*x;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The standard's IMDCT: `y[i] = sum_k x[k] cos(pi/72 (2i + 1 + 18)(2k + 1))`, windowed.
    fn direct(x: &[f32; 18], window: impl Fn(usize) -> f64) -> [f64; 36] {
        core::array::from_fn(|i| {
            let y: f64 = (0..18)
                .map(|k| {
                    let a = core::f64::consts::PI / 72.0 * ((2 * i + 1 + 18) * (2 * k + 1)) as f64;
                    f64::from(x[k]) * libm::cos(a)
                })
                .sum();
            y * window(i)
        })
    }

    #[test]
    fn imdct36_is_the_windowed_imdct_of_the_standard_split_over_two_granules() {
        let normal = |i: usize| libm::sin(core::f64::consts::PI / 36.0 * (i as f64 + 0.5));
        let mut seed = 7u32;
        for _ in 0..20 {
            let x: [f32; 18] = core::array::from_fn(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 8) as f32 / (1 << 23) as f32 - 1.0
            });
            let want = direct(&x, normal);
            let mut buf = [0f32; 576];
            buf[..18].copy_from_slice(&x);
            let mut overlap = [0f32; 288];
            imdct(&mut buf, &mut overlap, 0, 0, 32);
            let first: Vec<f64> = buf[..18].iter().map(|&v| f64::from(v)).collect();
            let mut next = [0f32; 576];
            imdct(&mut next, &mut overlap, 0, 0, 32);
            let second: Vec<f64> = next[..18].iter().map(|&v| f64::from(v)).collect();
            for i in 0..18 {
                assert!(
                    (first[i] - want[i]).abs() < 1e-5,
                    "{i}: {} {}",
                    first[i],
                    want[i]
                );
                assert!(
                    (second[i] - want[18 + i]).abs() < 1e-5,
                    "{i}: {} {}",
                    second[i],
                    want[18 + i]
                );
            }
        }
    }
}
