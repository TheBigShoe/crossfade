/* Copyright (c) 2011-2012 Xiph.Org Foundation, Mozilla Corporation
Written by Jean-Marc Valin and Timothy B. Terriberry */
/*
   Redistribution and use in source and binary forms, with or without
   modification, are permitted provided that the following conditions
   are met:

   - Redistributions of source code must retain the above copyright
   notice, this list of conditions and the following disclaimer.

   - Redistributions in binary form must reproduce the above copyright
   notice, this list of conditions and the following disclaimer in the
   documentation and/or other materials provided with the distribution.

   THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
   ``AS IS'' AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
   LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
   A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER
   OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL,
   EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO,
   PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR
   PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF
   LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
   NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
   SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
*/

//! A port of `opus_compare.c`, the test vectors' quality measure: a pseudo noise-to-mask ratio
//! in the 21 CELT bands of 480-sample Hann windows every 2.5 ms, with frequency, temporal and
//! cross-channel masking, folded into one figure Q. A vector passes when Q >= 0; RFC 6716
//! recommends Q > 90 at 48 kHz. Kept in the C's float and double types and order of
//! operations, with `libm` for every transcendental, so it gives the same figure everywhere.

// OPUS_PI, 3.14159265F, rounds to the same float.
const PI: f32 = std::f32::consts::PI;
const NBANDS: usize = 21;
const NFREQS: usize = 240;
/// Bands on which the pseudo-NMR is computed (Bark-derived CELT bands).
const BANDS: [usize; NBANDS + 1] = [
    0, 2, 4, 6, 8, 10, 12, 14, 16, 20, 24, 28, 32, 40, 48, 56, 68, 80, 96, 120, 156, 200,
];
const TEST_WIN_SIZE: usize = 480;
const TEST_WIN_STEP: usize = 120;

/// The outcome of a comparison.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quality {
    /// The quality figure, at most 100; the vector passes when it is at least 0.
    pub q: f32,
    /// The internal weighted error Q is computed from.
    pub err: f64,
}

impl Quality {
    pub fn passes(&self) -> bool {
        self.q >= 0.0
    }
}

/// `opus_compare [-s] [-r rate] reference decoded` on 16-bit samples: `reference` is always
/// 48 kHz stereo, interleaved (as the `.dec` files are); `decoded` is stereo (`stereo`) or
/// mono at `rate` (8, 12, 16, 24 or 48 kHz). The error texts are the tool's.
pub fn compare_pcm16(
    reference: &[i16],
    decoded: &[i16],
    stereo: bool,
    rate: u32,
) -> Result<Quality, String> {
    let x: Vec<f32> = reference.iter().map(|&s| f32::from(s)).collect();
    let y: Vec<f32> = decoded.iter().map(|&s| f32::from(s)).collect();
    compare(&x, &y, stereo, rate)
}

/// [`compare_pcm16`] on samples already in the 16-bit scale (as floats, which is what the tool
/// computes with).
pub fn compare(
    reference: &[f32],
    decoded: &[f32],
    stereo: bool,
    rate: u32,
) -> Result<Quality, String> {
    let nchannels = if stereo { 2 } else { 1 };
    let (downsample, ybands) = match rate {
        48000 => (1, NBANDS),
        24000 => (2, 19),
        16000 => (3, 17),
        12000 => (4, 15),
        8000 => (6, 13),
        _ => return Err("Sampling rate must be 8000, 12000, 16000, 24000, or 48000".into()),
    };
    let yfreqs = NFREQS / downsample;
    // read_pcm16 reads whole frames: the reference as stereo, the decoded file as nchannels.
    let xlength = reference.len() / 2;
    let mut x: Vec<f32> = reference[..2 * xlength].to_vec();
    if nchannels == 1 {
        for xi in 0..xlength {
            x[xi] = (0.5 * (f64::from(x[2 * xi]) + f64::from(x[2 * xi + 1]))) as f32;
        }
        x.truncate(xlength);
    }
    let ylength = decoded.len() / nchannels;
    let y = &decoded[..ylength * nchannels];
    if xlength != ylength * downsample {
        return Err(format!(
            "Sample counts do not match ({xlength}!={}).",
            ylength * downsample
        ));
    }
    if xlength < TEST_WIN_SIZE {
        return Err(format!(
            "Insufficient sample data ({xlength}<{TEST_WIN_SIZE})."
        ));
    }
    let nframes = (xlength - TEST_WIN_SIZE + TEST_WIN_STEP) / TEST_WIN_STEP;
    let mut xb = vec![0f32; nframes * NBANDS * nchannels];
    let mut xs = vec![0f32; nframes * NFREQS * nchannels];
    let mut ys = vec![0f32; nframes * yfreqs * nchannels];
    // The per-band spectral energy of the original signal and the error.
    band_energy(
        Some(&mut xb),
        &mut xs,
        &BANDS,
        NBANDS,
        &x,
        nchannels,
        nframes,
        TEST_WIN_SIZE,
        TEST_WIN_STEP,
        1,
    );
    band_energy(
        None,
        &mut ys,
        &BANDS,
        ybands,
        y,
        nchannels,
        nframes,
        TEST_WIN_SIZE / downsample,
        TEST_WIN_STEP / downsample,
        downsample,
    );
    for xi in 0..nframes {
        // Frequency masking (low to high): 10 dB/Bark slope.
        for bi in 1..NBANDS {
            for ci in 0..nchannels {
                xb[(xi * NBANDS + bi) * nchannels + ci] +=
                    0.1 * xb[(xi * NBANDS + bi - 1) * nchannels + ci];
            }
        }
        // Frequency masking (high to low): 15 dB/Bark slope.
        for bi in (0..NBANDS - 1).rev() {
            for ci in 0..nchannels {
                xb[(xi * NBANDS + bi) * nchannels + ci] +=
                    0.03 * xb[(xi * NBANDS + bi + 1) * nchannels + ci];
            }
        }
        if xi > 0 {
            // Temporal masking: -3 dB/2.5ms slope.
            for bi in 0..NBANDS {
                for ci in 0..nchannels {
                    xb[(xi * NBANDS + bi) * nchannels + ci] +=
                        0.5 * xb[((xi - 1) * NBANDS + bi) * nchannels + ci];
                }
            }
        }
        // Allowing some cross-talk.
        if nchannels == 2 {
            for bi in 0..NBANDS {
                let l = xb[(xi * NBANDS + bi) * nchannels];
                let r = xb[(xi * NBANDS + bi) * nchannels + 1];
                xb[(xi * NBANDS + bi) * nchannels] += 0.01 * r;
                xb[(xi * NBANDS + bi) * nchannels + 1] += 0.01 * l;
            }
        }
        // Apply masking.
        for bi in 0..ybands {
            for xj in BANDS[bi]..BANDS[bi + 1] {
                for ci in 0..nchannels {
                    let m = 0.1 * xb[(xi * NBANDS + bi) * nchannels + ci];
                    xs[(xi * NFREQS + xj) * nchannels + ci] += m;
                    ys[(xi * yfreqs + xj) * nchannels + ci] += m;
                }
            }
        }
    }
    // Average of consecutive frames to make comparison slightly less sensitive.
    for bi in 0..ybands {
        for xj in BANDS[bi]..BANDS[bi + 1] {
            for ci in 0..nchannels {
                let mut xtmp = xs[xj * nchannels + ci];
                let mut ytmp = ys[xj * nchannels + ci];
                for xi in 1..nframes {
                    let xtmp2 = xs[(xi * NFREQS + xj) * nchannels + ci];
                    let ytmp2 = ys[(xi * yfreqs + xj) * nchannels + ci];
                    xs[(xi * NFREQS + xj) * nchannels + ci] += xtmp;
                    ys[(xi * yfreqs + xj) * nchannels + ci] += ytmp;
                    xtmp = xtmp2;
                    ytmp = ytmp2;
                }
            }
        }
    }
    // At a lower rate, the last 300 Hz are left out to allow for different transition bands;
    // at 12 kHz nothing is, as the last band already leaves out 400 Hz.
    let max_compare = match rate {
        48000 => BANDS[NBANDS],
        12000 => BANDS[ybands],
        _ => BANDS[ybands] - 3,
    };
    let mut err = 0f64;
    for xi in 0..nframes {
        let mut ef = 0f64;
        for bi in 0..ybands {
            let mut eb = 0f64;
            for xj in BANDS[bi]..BANDS[bi + 1].min(max_compare) {
                for ci in 0..nchannels {
                    let re = ys[(xi * yfreqs + xj) * nchannels + ci]
                        / xs[(xi * NFREQS + xj) * nchannels + ci];
                    let mut im = (f64::from(re) - libm::log(f64::from(re)) - 1.0) as f32;
                    // Less sensitive around the SILK/CELT cross-over, to allow for mode
                    // freedom in the filters.
                    if (79..=81).contains(&xj) {
                        im *= 0.1;
                    }
                    if xj == 80 {
                        im *= 0.1;
                    }
                    eb += f64::from(im);
                }
            }
            eb /= ((BANDS[bi + 1] - BANDS[bi]) * nchannels) as f64;
            ef += eb * eb;
        }
        // A fixed normalisation: slightly lower quality is accepted at lower rates.
        ef /= NBANDS as f64;
        ef *= ef;
        err += ef * ef;
    }
    let err = libm::pow(err / nframes as f64, 1.0 / 16.0);
    let q = (100.0 * (1.0 - 0.5 * libm::log(1.0 + err) / libm::log(1.13))) as f32;
    Ok(Quality { q, err })
}

#[allow(clippy::too_many_arguments)]
fn band_energy(
    mut out: Option<&mut [f32]>,
    ps: &mut [f32],
    bands: &[usize],
    nbands: usize,
    input: &[f32],
    nchannels: usize,
    nframes: usize,
    window_sz: usize,
    step: usize,
    downsample: usize,
) {
    let cosf = |x: f32| libm::cos(f64::from(x)) as f32;
    let sinf = |x: f32| libm::sin(f64::from(x)) as f32;
    let ps_sz = window_sz / 2;
    let window: Vec<f32> = (0..window_sz)
        .map(|xj| 0.5 - 0.5 * cosf((2.0 * PI / (window_sz - 1) as f32) * xj as f32))
        .collect();
    let c: Vec<f32> = (0..window_sz)
        .map(|xj| cosf((2.0 * PI / window_sz as f32) * xj as f32))
        .collect();
    let s: Vec<f32> = (0..window_sz)
        .map(|xj| sinf((2.0 * PI / window_sz as f32) * xj as f32))
        .collect();
    let mut x = vec![0f32; nchannels * window_sz];
    for xi in 0..nframes {
        for ci in 0..nchannels {
            for xk in 0..window_sz {
                x[ci * window_sz + xk] = window[xk] * input[(xi * step + xk) * nchannels + ci];
            }
        }
        let mut xj = 0;
        for bi in 0..nbands {
            let mut p = [0f32; 2];
            while xj < bands[bi + 1] {
                for ci in 0..nchannels {
                    let mut re = 0f32;
                    let mut im = 0f32;
                    let mut ti = 0;
                    for xk in 0..window_sz {
                        re += c[ti] * x[ci * window_sz + xk];
                        im -= s[ti] * x[ci * window_sz + xk];
                        ti += xj;
                        if ti >= window_sz {
                            ti -= window_sz;
                        }
                    }
                    re *= downsample as f32;
                    im *= downsample as f32;
                    let v = re * re + im * im + 100000.0;
                    ps[(xi * ps_sz + xj) * nchannels + ci] = v;
                    p[ci] += v;
                }
                xj += 1;
            }
            if let Some(out) = out.as_deref_mut() {
                let width = (bands[bi + 1] - bands[bi]) as f32;
                out[(xi * nbands + bi) * nchannels] = p[0] / width;
                if nchannels == 2 {
                    out[(xi * nbands + bi) * nchannels + 1] = p[1] / width;
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "compare_tests.rs"]
mod tests;
