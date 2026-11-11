//! Streaming properties: the output bits do not depend on how the input is split, channels
//! are independent, non-finite input stays local, and the output of a fixed conversion is
//! pinned (the same bits on every platform).
mod common;

use common::*;
use crossfade_resample::{MAX_RATE, Quality, Resampler};
use proptest::prelude::*;

/// Direct form (exact and interpolated), transposed form (exact and interpolated).
const PAIRS: [(u32, u32); 8] = [
    (44_100, 48_000),
    (48_000, 44_100),
    (44_100, 44_101),
    (44_101, 44_100),
    (8_000, 96_000),
    (192_000, 1_000),
    (MAX_RATE, 7_000),
    (1_000, 44_101),
];

fn bits(v: &[f64]) -> Vec<u64> {
    v.iter().map(|x| x.to_bits()).collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn output_bits_do_not_depend_on_chunking(
        pair in 0..PAIRS.len(),
        q in 0..3usize,
        channels in 1..=3usize,
        frames in 0..2500usize,
        cuts in prop::collection::vec(0..400usize, 1..40),
        planar in any::<bool>(),
        seed in any::<u64>(),
    ) {
        let (a, b) = PAIRS[pair];
        let q = QUALITIES[q];
        let x = noise(frames * channels, seed);
        let whole = convert(a, b, channels, q, &x, frames.max(1));

        let mut r = Resampler::new(a, b, channels, q).unwrap();
        let mut out = Vec::new();
        let mut planes = vec![Vec::new(); channels];
        let mut at = 0;
        for &cut in cuts.iter().cycle() {
            if at == frames {
                break;
            }
            let n = cut.min(frames - at);
            let chunk = &x[at * channels..(at + n) * channels];
            if planar {
                let chans: Vec<Vec<f64>> = (0..channels).map(|c| channel(chunk, channels, c)).collect();
                let refs: Vec<&[f64]> = chans.iter().map(Vec::as_slice).collect();
                r.process_planar(&refs, &mut planes).unwrap();
            } else {
                r.process(chunk, &mut out).unwrap();
            }
            at += n;
        }
        if planar {
            r.flush_planar(&mut planes).unwrap();
            for (c, plane) in planes.iter().enumerate() {
                prop_assert_eq!(bits(plane), bits(&channel(&whole, channels, c)));
            }
        } else {
            r.flush(&mut out);
            prop_assert_eq!(bits(&out), bits(&whole));
        }
    }
}

#[test]
fn channels_do_not_leak_into_each_other() {
    let frames = 4000;
    for (a, b) in PAIRS {
        let x = noise(frames, 5);
        let mut four = vec![0.0; frames * 4];
        for (f, &v) in x.iter().enumerate() {
            four[f * 4 + 2] = v;
        }
        let out = convert(a, b, 4, Quality::Balanced, &four, 999);
        let alone = mono(a, b, Quality::Balanced, &x);
        assert_eq!(bits(&channel(&out, 4, 2)), bits(&alone), "{a} -> {b}");
        for c in [0, 1, 3] {
            assert!(
                channel(&out, 4, c).iter().all(|&v| v == 0.0),
                "{a} -> {b}: channel {c}"
            );
        }
    }
}

/// A NaN or an infinity spoils only the outputs whose kernel reaches it; the stream then
/// recovers, bit for bit, and the length rule holds.
#[test]
fn non_finite_input_stays_within_the_kernel_reach() {
    for (a, b) in PAIRS {
        let lat = Resampler::new(a, b, 1, Quality::Fast).unwrap().latency() as usize;
        let n = 6 * lat + 3000;
        let k = n / 2;
        let clean = noise(n, 3);
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut x = clean.clone();
            x[k] = bad;
            let y = mono(a, b, Quality::Fast, &x);
            let want = mono(a, b, Quality::Fast, &clean);
            assert_eq!(y.len(), want.len());
            // Output instant of input k, and the kernel's reach in output frames.
            let centre = k as f64 * f64::from(b) / f64::from(a);
            let reach = (lat + 2) as f64 * f64::from(b) / f64::from(a) + 2.0;
            for (n, (p, q)) in y.iter().zip(&want).enumerate() {
                if (n as f64 - centre).abs() > reach {
                    assert_eq!(p.to_bits(), q.to_bits(), "{a} -> {b} {bad}: output {n}");
                }
            }
            assert!(y.iter().any(|v| !v.is_finite()), "{a} -> {b} {bad}");
        }
    }
}

/// FNV-1a over the output's little-endian bytes.
fn fnv1a(v: &[f64]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for byte in v.iter().flat_map(|x| x.to_le_bytes()) {
        h = (h ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// The design (libm only) and the kernels (fixed summation order, no fused multiply-add) give
/// the same bits on every platform; this pins them for one conversion per form.
#[test]
fn output_bits_are_pinned() {
    let x = noise(2 * 5000, 2026);
    let cases = [
        (44_100, 48_000, Quality::Balanced, 0x993e_690c_4e20_752c),
        (48_000, 44_100, Quality::Best, 0x811e_b8cf_c911_6b69),
        (44_100, 44_101, Quality::Fast, 0x23c7_b604_4f99_7078),
        (192_000, 1_000, Quality::Balanced, 0xa62d_757e_1b62_a567),
        (MAX_RATE, 7_000, Quality::Balanced, 0x42cb_2102_28ce_bb27),
    ];
    let got: Vec<u64> = cases
        .iter()
        .map(|&(a, b, q, _)| fnv1a(&convert(a, b, 2, q, &x, 1234)))
        .collect();
    let want: Vec<u64> = cases.iter().map(|c| c.3).collect();
    assert_eq!(got, want, "{got:#018x?}");
}
