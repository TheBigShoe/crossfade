//! Rate pairs whose reduced ratio has a huge numerator or denominator: they construct within
//! the memory bound, keep the length rule, pass DC and keep the stated quality.
mod common;

use common::*;
use crossfade_resample::{MAX_RATE, Quality, Resampler, output_len};

/// Upper bound on [`Resampler::memory_bytes`] for any rates and quality, stereo: tables plus
/// history (see the README).
const BOUND: usize = 4 << 20;

const AWKWARD: [(u32, u32); 12] = [
    (44_100, 44_101),
    (44_101, 44_100),
    (8_000, 192_000),
    (192_000, 8_000),
    (MAX_RATE, 1),
    (1, MAX_RATE),
    (MAX_RATE, 7),
    (7, MAX_RATE),
    (MAX_RATE, 7_000),
    (999_983, 1_000_003),
    (1_000_003, 999_983),
    (96_001, 48_000),
];

#[test]
fn every_pair_constructs_within_the_memory_bound_and_keeps_the_length_rule() {
    for (a, b) in AWKWARD {
        for q in QUALITIES {
            let mut r = Resampler::new(a, b, 2, q).unwrap();
            assert!(
                r.memory_bytes() <= BOUND,
                "{a} -> {b} {q:?}: {} bytes",
                r.memory_bytes()
            );
            // One input frame makes a million output frames here: run it once, at Fast,
            // so a debug build stays quick (the kernels are the same at every quality).
            let frames = if b / a > 1000 { 1 } else { 3000 };
            if frames == 1 && q != Quality::Fast {
                continue;
            }
            let x = noise(2 * frames, 9);
            let mut out = Vec::new();
            for c in x.chunks(2 * 701) {
                r.process(c, &mut out).unwrap();
            }
            r.flush(&mut out);
            assert_eq!(
                out.len() as u64,
                2 * output_len(frames as u64, a, b),
                "{a} -> {b} {q:?}"
            );
            assert!(out.iter().all(|v| v.is_finite()), "{a} -> {b} {q:?}");
        }
    }
}

#[test]
fn memory_does_not_grow_with_the_stream() {
    for (a, b) in [(44_100, 44_101), (MAX_RATE, 7_000), (44_100, 48_000)] {
        let mut r = Resampler::new(a, b, 2, Quality::Fast).unwrap();
        let before = r.memory_bytes();
        let mut out = Vec::new();
        for _ in 0..20 {
            r.process(&noise(2 * 4000, 1), &mut out).unwrap();
            out.clear();
        }
        assert_eq!(r.memory_bytes(), before, "{a} -> {b}");
    }
}

#[test]
fn dc_gain_is_one_within_1e_12_for_interpolated_phases() {
    for (a, b, n) in [
        (44_100, 44_101, 6000),
        (44_101, 44_100, 6000),
        (999_983, 1_000_003, 6000),
        (MAX_RATE, 7_000, 120_000),
    ] {
        for q in QUALITIES {
            let out = mono(a, b, q, &vec![0.7; n]);
            let edge = out.len() / 3;
            for &y in steady(&out, edge) {
                assert!((y / 0.7 - 1.0).abs() < 1e-12, "{a} -> {b} {q:?}: {y}");
            }
        }
    }
}

/// Interpolating between table rows must not cost the stated attenuation.
#[test]
fn interpolated_phases_keep_the_stopband() {
    for (q, stop) in [
        (Quality::Fast, 100.0),
        (Quality::Balanced, 120.0),
        (Quality::Best, 160.0),
    ] {
        for (a, b) in [(44_100, 44_101), (44_101, 44_100)] {
            for f in [997.0, 10_007.0, 18_007.0] {
                let y = mono(a, b, q, &sine(8192, f, a.into(), 0.5));
                let (amp, res) = fit(steady(&y, 400), 400, f, b.into());
                assert!((amp / 0.5 - 1.0).abs() < 1e-4, "{q:?} {a} -> {b} {f}");
                let level = db(res / (0.5 / 2f64.sqrt()));
                assert!(level <= -stop, "{q:?} {a} -> {b} {f}: {level:.1} dB");
            }
        }
    }
}

/// Heavy decimation (the transposed form) still removes everything above the new Nyquist
/// frequency and passes the band below it.
#[test]
fn heavy_decimation_filters_and_passes() {
    let (a, b) = (192_000, 1_000);
    for (q, stop) in [
        (Quality::Fast, 100.0),
        (Quality::Balanced, 120.0),
        (Quality::Best, 160.0),
    ] {
        let lat = Resampler::new(a, b, 1, q).unwrap().latency() as usize;
        let n = 8 * lat;
        let edge = lat * b as usize / a as usize + 8;
        let y = mono(a, b, q, &sine(n, 200.0, a.into(), 0.5));
        let (amp, res) = fit(steady(&y, edge), edge, 200.0, b.into());
        assert!((amp / 0.5 - 1.0).abs() < 1e-6, "{q:?}: {amp}");
        assert!(db(res / 0.35) <= -stop, "{q:?}: {:.1}", db(res / 0.35));
        let y = mono(a, b, q, &sine(n, 503.0, a.into(), 0.5));
        let level = db(rms(steady(&y, edge)) / 0.35);
        assert!(level <= -stop, "{q:?}: alias at {level:.1} dB");
    }
}
