//! The inverse MDCT (specification section 1.3.2), through a DCT-IV of half the block computed
//! with a complex FFT of a quarter of it. The IMDCT's output is the DCT-IV's folded out
//! ([`unfold`]); the decoder reads it from the DCT-IV directly and never builds the whole
//! block. Tables come from `libm` in `f64`, so every platform gets the same bits.
//!
//! With `M = n/2` coefficients `X` and `L = M/2`:
//! `c[j] = (X[2j] + i X[M-1-2j]) e^{-i pi j/M}`, `F = FFT_L(c)`,
//! `Z[p] = F[p] e^{-i pi (p+1/4)/M}`, and the DCT-IV is `z[2p] = Re Z[p]`,
//! `z[M-1-2p] = -Im Z[p]`.

use std::f64::consts::PI;

/// Work space for the largest block a decoder uses; it serves any smaller one.
#[derive(Debug, Clone)]
pub(crate) struct Scratch {
    re: Vec<f32>,
    im: Vec<f32>,
}

impl Scratch {
    pub(crate) fn new(n: usize) -> Self {
        Self {
            re: vec![0.0; n / 4],
            im: vec![0.0; n / 4],
        }
    }

    pub(crate) fn bytes(n: usize) -> usize {
        n / 4 * 8
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Imdct {
    /// The block size.
    n: usize,
    /// `e^{-i pi j/M}` for the pre-rotation, `j < L`.
    pre_cos: Vec<f32>,
    pre_sin: Vec<f32>,
    /// `e^{-i pi (p+1/4)/M}` for the post-rotation, `p < L`.
    post_cos: Vec<f32>,
    post_sin: Vec<f32>,
    /// Where each input of the FFT goes (bit reversal).
    bitrev: Vec<u16>,
    /// The FFT's twiddles, `e^{-2 pi i k/(2h)}` at `h + k` for each stage's half size `h`.
    tw_re: Vec<f32>,
    tw_im: Vec<f32>,
}

impl Imdct {
    /// `n` is a power of two from 64 to 8192.
    pub(crate) fn new(n: usize) -> Self {
        let m = n / 2;
        let l = n / 4;
        let bits = l.trailing_zeros();
        let angle = |num: f64, den: f64| {
            (
                libm::cos(PI * num / den) as f32,
                libm::sin(PI * num / den) as f32,
            )
        };
        let (pre_cos, pre_sin) = (0..l).map(|j| angle(j as f64, m as f64)).unzip();
        let (post_cos, post_sin) = (0..l).map(|p| angle(p as f64 + 0.25, m as f64)).unzip();
        let bitrev = (0..l)
            .map(|j| ((j as u32).reverse_bits() >> (32 - bits)) as u16)
            .collect();
        let mut tw_re = vec![0.0; l.max(2)];
        let mut tw_im = vec![0.0; l.max(2)];
        let mut h = 1;
        while h < l {
            for k in 0..h {
                let (c, s) = angle(k as f64, h as f64);
                tw_re[h + k] = c;
                tw_im[h + k] = -s;
            }
            h *= 2;
        }
        Self {
            n,
            pre_cos,
            pre_sin,
            post_cos,
            post_sin,
            bitrev,
            tw_re,
            tw_im,
        }
    }

    pub(crate) fn bytes(n: usize) -> usize {
        // Six tables of n/4 floats (or u16s) and the twiddles.
        n / 4 * (4 * 4 + 2 + 2 * 4) + 16
    }

    /// The DCT-IV of `x` (the block's `n/2` coefficients), in place.
    pub(crate) fn dct4(&self, x: &mut [f32], scratch: &mut Scratch) {
        let m = self.n / 2;
        let l = self.n / 4;
        let x = &mut x[..m];
        let re = &mut scratch.re[..l];
        let im = &mut scratch.im[..l];
        for j in 0..l {
            let a = x[2 * j];
            let b = x[m - 1 - 2 * j];
            let (c, s) = (self.pre_cos[j], self.pre_sin[j]);
            let t = usize::from(self.bitrev[j]);
            re[t] = a * c + b * s;
            im[t] = b * c - a * s;
        }
        fft_in_place(re, im, &self.tw_re, &self.tw_im);
        for p in 0..l {
            let (c, s) = (self.post_cos[p], self.post_sin[p]);
            let (fr, fi) = (re[p], im[p]);
            x[2 * p] = fr * c + fi * s;
            x[m - 1 - 2 * p] = fr * s - fi * c;
        }
    }
}

/// A radix-2 decimation-in-time FFT over input already in bit-reversed order.
fn fft_in_place(re: &mut [f32], im: &mut [f32], tw_re: &[f32], tw_im: &[f32]) {
    let l = re.len();
    // Half size 1: twiddle 1.
    for (r, i) in re
        .as_chunks_mut::<2>()
        .0
        .iter_mut()
        .zip(im.as_chunks_mut::<2>().0)
    {
        let (ar, ai, br, bi) = (r[0], i[0], r[1], i[1]);
        r[0] = ar + br;
        i[0] = ai + bi;
        r[1] = ar - br;
        i[1] = ai - bi;
    }
    // Half size 2: twiddles 1 and -i.
    for (r, i) in re
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(im.as_chunks_mut::<4>().0)
    {
        let (a0r, a0i, b0r, b0i) = (r[0], i[0], r[2], i[2]);
        r[0] = a0r + b0r;
        i[0] = a0i + b0i;
        r[2] = a0r - b0r;
        i[2] = a0i - b0i;
        // (br + i bi)(-i) = bi - i br
        let (a1r, a1i, b1r, b1i) = (r[1], i[1], i[3], -r[3]);
        r[1] = a1r + b1r;
        i[1] = a1i + b1i;
        r[3] = a1r - b1r;
        i[3] = a1i - b1i;
    }
    let mut h = 4;
    while h < l {
        let (wr, wi) = (&tw_re[h..2 * h], &tw_im[h..2 * h]);
        for (r, i) in re.chunks_exact_mut(2 * h).zip(im.chunks_exact_mut(2 * h)) {
            let (r0, r1) = r.split_at_mut(h);
            let (i0, i1) = i.split_at_mut(h);
            butterflies(r0, i0, r1, i1, wr, wi);
        }
        h *= 2;
    }
}

#[inline(always)]
fn butterflies(
    r0: &mut [f32],
    i0: &mut [f32],
    r1: &mut [f32],
    i1: &mut [f32],
    wr: &[f32],
    wi: &[f32],
) {
    let h = r0.len();
    let (i0, r1, i1, wr, wi) = (&mut i0[..h], &mut r1[..h], &mut i1[..h], &wr[..h], &wi[..h]);
    for k in 0..h {
        let br = r1[k] * wr[k] - i1[k] * wi[k];
        let bi = r1[k] * wi[k] + i1[k] * wr[k];
        let (ar, ai) = (r0[k], i0[k]);
        r0[k] = ar + br;
        i0[k] = ai + bi;
        r1[k] = ar - br;
        i1[k] = ai - bi;
    }
}

/// The IMDCT of a block from the DCT-IV `z` of its coefficients (`n = 2 z.len()` samples).
#[cfg(test)]
pub(crate) fn unfold(z: &[f32]) -> Vec<f32> {
    let m = z.len();
    let n = 2 * m;
    (0..n)
        .map(|i| {
            if i < m / 2 {
                z[i + m / 2]
            } else if i < 3 * m / 2 {
                -z[3 * m / 2 - 1 - i]
            } else {
                -z[i - 3 * m / 2]
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "mdct_tests.rs"]
mod tests;
