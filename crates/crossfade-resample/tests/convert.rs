//! The time-domain contract: length, latency, time alignment, DC gain.
mod common;

use common::*;
use crossfade_resample::{Quality, Resampler, output_len};

/// Common pairs, both directions, with small and large phase counts.
const PAIRS: [(u32, u32); 10] = [
    (44_100, 48_000),
    (48_000, 44_100),
    (44_100, 96_000),
    (96_000, 44_100),
    (192_000, 48_000),
    (48_000, 192_000),
    (8_000, 44_100),
    (44_100, 88_200),
    (88_200, 44_100),
    (22_050, 16_000),
];

#[test]
fn output_length_is_ceil_of_frames_times_the_ratio() {
    for (a, b) in PAIRS {
        for n in [0usize, 1, 2, 3, 10, 147, 160, 1000, 4097] {
            for q in QUALITIES {
                let out = convert(a, b, 2, q, &noise(2 * n, 1), 333);
                assert_eq!(
                    out.len() as u64,
                    2 * output_len(n as u64, a, b),
                    "{a} -> {b}, {n} frames, {q:?}"
                );
            }
        }
    }
}

#[test]
fn nothing_comes_out_until_latency_plus_one_frames_went_in() {
    for (a, b) in PAIRS {
        for q in QUALITIES {
            let mut r = Resampler::new(a, b, 1, q).unwrap();
            let lat = r.latency() as usize;
            assert!(lat > 0, "{a} -> {b} {q:?}");
            let mut out = Vec::new();
            r.process(&vec![0.5; lat], &mut out).unwrap();
            assert!(out.is_empty(), "{a} -> {b} {q:?}: {} early", out.len());
            r.process(&[0.5], &mut out).unwrap();
            assert!(!out.is_empty(), "{a} -> {b} {q:?}");
        }
    }
}

#[test]
fn dc_gain_is_one_within_1e_12() {
    for (a, b) in PAIRS {
        for q in QUALITIES {
            let out = mono(a, b, q, &vec![0.7; 30_000]);
            let edge = out.len() / 4;
            for (k, &y) in steady(&out, edge).iter().enumerate() {
                assert!(
                    (y / 0.7 - 1.0).abs() < 1e-12,
                    "{a} -> {b} {q:?}: output {} is {y}",
                    k + edge
                );
            }
        }
    }
}

/// An impulse at an input instant that lands exactly on an output instant comes out
/// centred there, its peak at that output and symmetric about it.
#[test]
fn impulse_response_is_symmetric_and_time_aligned() {
    for (a, b) in PAIRS {
        for q in QUALITIES {
            let g = gcd(a, b);
            let (step_in, step_out) = ((a / g) as usize, (b / g) as usize);
            let k = step_in * (2000 / step_in + 1);
            let centre = k / step_in * step_out;
            let mut x = vec![0.0; k * 2 + 4000];
            x[k] = 1.0;
            let y = mono(a, b, q, &x);
            let peak = (0..y.len())
                .max_by(|&i, &j| y[i].abs().total_cmp(&y[j].abs()))
                .unwrap();
            assert_eq!(peak, centre, "{a} -> {b} {q:?}");
            let scale = y[centre].abs();
            for m in 1..centre.min(y.len() - centre - 1) {
                assert!(
                    (y[centre + m] - y[centre - m]).abs() <= 1e-15 * scale,
                    "{a} -> {b} {q:?}: asymmetric at {m}: {} vs {}",
                    y[centre + m],
                    y[centre - m]
                );
            }
        }
    }
}

#[test]
fn reset_and_flush_start_the_stream_afresh() {
    let x = noise(5000, 7);
    for (a, b) in PAIRS {
        let once = mono(a, b, Quality::Balanced, &x);
        let mut r = Resampler::new(a, b, 1, Quality::Balanced).unwrap();
        let mut out = Vec::new();
        r.process(&noise(777, 3), &mut out).unwrap();
        r.reset();
        out.clear();
        r.process(&x, &mut out).unwrap();
        r.flush(&mut out);
        assert_eq!(out, once, "{a} -> {b} after reset");
        out.clear();
        r.process(&x, &mut out).unwrap();
        r.flush(&mut out);
        assert_eq!(out, once, "{a} -> {b} after flush");
    }
}

#[test]
fn planar_and_interleaved_give_the_same_bits() {
    let left = noise(3000, 1);
    let right = noise(3000, 2);
    let inter: Vec<f64> = left.iter().zip(&right).flat_map(|(&l, &r)| [l, r]).collect();
    for (a, b) in PAIRS {
        let want = convert(a, b, 2, Quality::Balanced, &inter, 1000);
        let mut r = Resampler::new(a, b, 2, Quality::Balanced).unwrap();
        let mut out = vec![Vec::new(), Vec::new()];
        for (l, rr) in left.chunks(700).zip(right.chunks(700)) {
            r.process_planar(&[l, rr], &mut out).unwrap();
        }
        r.flush_planar(&mut out).unwrap();
        assert_eq!(out[0], channel(&want, 2, 0), "{a} -> {b}");
        assert_eq!(out[1], channel(&want, 2, 1), "{a} -> {b}");
    }
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}
