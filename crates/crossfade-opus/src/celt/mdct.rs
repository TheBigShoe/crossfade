//! The MDCT (`celt/mdct.c`, float build). Backward (`clt_mdct_backward`): pre-rotation, an
//! N/4-point complex inverse FFT, post-rotation, and the TDAC mirroring into the output with
//! the low-overlap window. Forward (`clt_mdct_forward`): window and fold, pre-rotation, the
//! forward FFT, post-rotation.

use super::fft::{Cpx, fft, ifft, size};
use super::tables::{MDCT_TRIG, OVERLAP, WINDOW};

/// The longest MDCT (`mdct.n`): two 20 ms frames.
const MDCT_N: usize = 1920;

/// `clt_mdct_backward`: `input` holds the `n / 2` coefficients of one MDCT of size
/// `n = 1920 >> shift`, every `stride`-th from the first; the windowed output is added into
/// `out` as the reference does from `out_base` (which may be negative: the reference's pointer
/// is moved back by `(n / 2 - overlap) / 2`). Writes stay within
/// `out_base + n/4 - overlap/2 .. out_base + n - (n/4 - overlap/2)`.
pub(super) fn backward(
    input: &[f32],
    stride: usize,
    out: &mut [f32],
    out_base: isize,
    shift: usize,
) {
    let n = MDCT_N >> shift;
    let n2 = n >> 1;
    let n4 = n >> 2;
    let overlap = OVERLAP;
    let trig = |k: usize| MDCT_TRIG[k << shift];
    let sine = (2.0 * std::f32::consts::PI * 0.125) / n as f32;

    let mut f2 = [Cpx::default(); 480];
    let mut f = [Cpx::default(); 480];
    let fft_n = size(shift);
    debug_assert_eq!(fft_n, n4);
    for i in 0..n4 {
        let x1 = input[stride * 2 * i];
        let x2 = input[stride * (n2 - 1 - 2 * i)];
        let yr = -(x2 * trig(i)) + x1 * trig(n4 - i);
        let yi = -(x2 * trig(n4 - i)) - x1 * trig(i);
        f2[i] = Cpx {
            r: yr - yi * sine,
            i: yi + yr * sine,
        };
    }
    ifft(shift, &f2[..n4], &mut f[..n4]);
    for (i, v) in f[..n4].iter_mut().enumerate() {
        let (re, im) = (v.r, v.i);
        let yr = re * trig(i) - im * trig(n4 - i);
        let yi = im * trig(i) + re * trig(n4 - i);
        *v = Cpx {
            r: yr - yi * sine,
            i: yi + yr * sine,
        };
    }
    // De-shuffle: f2 as a real array of n2 values.
    let mut g = [0.0f32; 960];
    {
        let fr = |k: usize| if k & 1 == 0 { f[k >> 1].r } else { f[k >> 1].i };
        for i in 0..n4 {
            g[2 * i] = -fr(2 * i);
            g[2 * i + 1] = fr(n2 - 1 - 2 * i);
        }
    }
    let base = out_base - ((n2 as isize - overlap as isize) >> 1);
    let at = |k: isize| (base + k) as usize;
    // Mirror on both sides for TDAC.
    {
        let mut fp1 = n4 as isize - 1;
        let mut xp1 = n2 as isize - 1;
        let mut yp1 = n4 as isize - (overlap / 2) as isize;
        let mut i = 0;
        while i < n4 - overlap / 2 {
            out[at(xp1)] = g[fp1 as usize];
            xp1 -= 1;
            fp1 -= 1;
            i += 1;
        }
        let mut w = 0;
        while i < n4 {
            let x1 = g[fp1 as usize];
            fp1 -= 1;
            out[at(yp1)] += -(WINDOW[w] * x1);
            out[at(xp1)] += WINDOW[overlap - 1 - w] * x1;
            yp1 += 1;
            xp1 -= 1;
            w += 1;
            i += 1;
        }
    }
    {
        let mut fp2 = n4;
        let mut xp2 = n2 as isize;
        let mut yp2 = n as isize - 1 - (n4 as isize - (overlap / 2) as isize);
        let mut i = 0;
        while i < n4 - overlap / 2 {
            out[at(xp2)] = g[fp2];
            xp2 += 1;
            fp2 += 1;
            i += 1;
        }
        let mut w = 0;
        while i < n4 {
            let x2 = g[fp2];
            fp2 += 1;
            out[at(yp2)] = WINDOW[w] * x2;
            out[at(xp2)] = WINDOW[overlap - 1 - w] * x2;
            yp2 -= 1;
            xp2 += 1;
            w += 1;
            i += 1;
        }
    }
}

/// `clt_mdct_forward`: the MDCT of size `n = 1920 >> shift` of `input`, `n / 2 + overlap`
/// samples (the block's first and last `(n / 2 - overlap) / 2` samples are zero and not
/// passed), windowed here; the `n / 2` coefficients go to every `stride`-th place of `out`,
/// scaled by `4 / n`.
pub(super) fn forward(input: &[f32], out: &mut [f32], stride: usize, shift: usize) {
    let n = MDCT_N >> shift;
    let n2 = n >> 1;
    let n4 = n >> 2;
    let overlap = OVERLAP;
    let half = overlap >> 1;
    let trig = |k: usize| MDCT_TRIG[k << shift];
    let sine = (2.0 * std::f32::consts::PI * 0.125) / n as f32;

    // The block as [a, b, c, d]: window, shuffle and fold into n / 2 values.
    let mut f = [Cpx::default(); 480];
    let quarter = (overlap + 3) >> 2;
    for (i, v) in f[..n4].iter_mut().enumerate() {
        let xp1 = half + 2 * i;
        let xp2 = n2 - 1 + half - 2 * i;
        *v = if i < quarter {
            // Real part -d - cR, imaginary part -b + aR.
            let w1 = WINDOW[half + 2 * i];
            let w2 = WINDOW[half - 1 - 2 * i];
            Cpx {
                r: w2 * input[xp1 + n2] + w1 * input[xp2],
                i: w1 * input[xp1] - w2 * input[xp2 - n2],
            }
        } else if i < n4 - quarter {
            Cpx {
                r: input[xp2],
                i: input[xp1],
            }
        } else {
            // Real part a - bR, imaginary part -c - dR.
            let j = i - (n4 - quarter);
            let w1 = WINDOW[2 * j];
            let w2 = WINDOW[overlap - 1 - 2 * j];
            Cpx {
                r: -(w1 * input[xp1 - n2]) + w2 * input[xp2],
                i: w2 * input[xp1] + w1 * input[xp2 + n2],
            }
        };
    }
    for (i, v) in f[..n4].iter_mut().enumerate() {
        let (re, im) = (v.r, v.i);
        let yr = -(re * trig(i)) - im * trig(n4 - i);
        let yi = -(im * trig(i)) + re * trig(n4 - i);
        *v = Cpx {
            r: yr + yi * sine,
            i: yi - yr * sine,
        };
    }
    let mut f2 = [Cpx::default(); 480];
    debug_assert_eq!(size(shift), n4);
    fft(shift, &f[..n4], &mut f2[..n4]);
    for (i, v) in f2[..n4].iter().enumerate() {
        let yr = v.i * trig(n4 - i) + v.r * trig(i);
        let yi = v.r * trig(n4 - i) - v.i * trig(i);
        out[stride * 2 * i] = yr - yi * sine;
        out[stride * (n2 - 1 - 2 * i)] = yi + yr * sine;
    }
}

#[cfg(test)]
#[path = "mdct_tests.rs"]
mod tests;
