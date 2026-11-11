//! Signals and measurements the tests share: no FFT, just least-squares tone fits.
#![allow(dead_code)]

use crossfade_resample::{Quality, Resampler};
use std::f64::consts::PI;

/// Converts `input` (interleaved) in chunks of `chunk` frames, flushing at the end.
pub(crate) fn convert(
    input_rate: u32,
    output_rate: u32,
    channels: usize,
    quality: Quality,
    input: &[f64],
    chunk: usize,
) -> Vec<f64> {
    let mut r = Resampler::new(input_rate, output_rate, channels, quality).unwrap();
    let mut out = Vec::new();
    for c in input.chunks(chunk.max(1) * channels) {
        r.process(c, &mut out).unwrap();
    }
    r.flush(&mut out);
    out
}

/// Mono shorthand for [`convert`] in one call.
pub(crate) fn mono(input_rate: u32, output_rate: u32, quality: Quality, input: &[f64]) -> Vec<f64> {
    convert(input_rate, output_rate, 1, quality, input, input.len())
}

/// Uniform noise in [-1, 1) from a 64-bit LCG: the same bits everywhere.
pub(crate) fn noise(n: usize, seed: u64) -> Vec<f64> {
    let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..n)
        .map(|_| {
            s = s
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (s >> 11) as f64 / (1u64 << 52) as f64 - 1.0
        })
        .collect()
}

/// `amp * sin(2 pi freq t)` sampled at `rate`, `n` frames.
pub(crate) fn sine(n: usize, freq: f64, rate: f64, amp: f64) -> Vec<f64> {
    (0..n)
        .map(|t| amp * libm::sin(2.0 * PI * freq * (t as f64) / rate))
        .collect()
}

/// Least-squares fit of `a cos + b sin` at `freq` to `x` (whose first sample is at index
/// `offset` of a signal sampled at `rate`): returns the amplitude and the residual's RMS.
pub(crate) fn fit(x: &[f64], offset: usize, freq: f64, rate: f64) -> (f64, f64) {
    let w = 2.0 * PI * freq / rate;
    let (mut cc, mut ss, mut cs, mut xc, mut xs) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (k, &v) in x.iter().enumerate() {
        let ph = w * (offset + k) as f64;
        let (c, s) = (libm::cos(ph), libm::sin(ph));
        cc += c * c;
        ss += s * s;
        cs += c * s;
        xc += v * c;
        xs += v * s;
    }
    let det = cc * ss - cs * cs;
    let a = (xc * ss - xs * cs) / det;
    let b = (xs * cc - xc * cs) / det;
    let mut res = 0.0;
    for (k, &v) in x.iter().enumerate() {
        let ph = w * (offset + k) as f64;
        let e = v - a * libm::cos(ph) - b * libm::sin(ph);
        res += e * e;
    }
    ((a * a + b * b).sqrt(), (res / x.len() as f64).sqrt())
}

pub(crate) fn rms(x: &[f64]) -> f64 {
    (x.iter().map(|v| v * v).sum::<f64>() / x.len() as f64).sqrt()
}

pub(crate) fn db(ratio: f64) -> f64 {
    20.0 * libm::log10(ratio)
}

/// Channel `ch` of interleaved `x`.
pub(crate) fn channel(x: &[f64], channels: usize, ch: usize) -> Vec<f64> {
    x.iter().skip(ch).step_by(channels).copied().collect()
}

/// The middle of `x` without `edge` samples at either end.
pub(crate) fn steady(x: &[f64], edge: usize) -> &[f64] {
    &x[edge..x.len() - edge]
}

pub(crate) const QUALITIES: [Quality; 3] = [Quality::Fast, Quality::Balanced, Quality::Best];
