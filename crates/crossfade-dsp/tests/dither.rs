//! Leaving the working format: TPDF dither from a fixed-seed generator, saturation with a
//! count, and floats left alone.

use crossfade_core::{AudioFrame, ChannelLayout, SampleFormat, Samples};
use crossfade_dsp::convert::convert_format;
use crossfade_dsp::dither::{Dither, Quantiser};
use proptest::prelude::*;

const INTS: [SampleFormat; 4] = [
    SampleFormat::U8,
    SampleFormat::S16,
    SampleFormat::S24,
    SampleFormat::S32,
];

fn quantise(format: SampleFormat, dither: Dither, x: &[f64]) -> (Samples, u64) {
    let mut q = Quantiser::new(format, dither);
    let mut out = Samples::new(SampleFormat::U8);
    q.quantise(x, &mut out);
    (out, q.clipped())
}

fn s16(x: &[f64], dither: Dither) -> (Vec<i16>, u64) {
    match quantise(SampleFormat::S16, dither, x) {
        (Samples::S16(v), c) => (v, c),
        other => panic!("{other:?}"),
    }
}

fn mean(v: impl Iterator<Item = f64> + Clone) -> f64 {
    let n = v.clone().count() as f64;
    v.sum::<f64>() / n
}

fn variance(v: impl Iterator<Item = f64> + Clone) -> f64 {
    let m = mean(v.clone());
    mean(v.map(|x| (x - m) * (x - m)))
}

const LSB: f64 = 1.0 / 32768.0;
const N: usize = 200_000;

#[test]
fn names_round_trip_and_tpdf_is_the_default() {
    assert_eq!(Dither::default(), Dither::Tpdf);
    for d in [Dither::Tpdf, Dither::None] {
        assert_eq!(Dither::from_name(d.name()), Some(d));
    }
    assert_eq!(Dither::NAMES, ["tpdf", "none"]);
    assert_eq!(Dither::from_name("TPDF"), None);
}

#[test]
fn without_dither_it_is_the_conversion_rule_bit_for_bit() {
    let x = [
        0.0, 0.5, -0.5, 0.999_99, -1.0, 1.0, 2.0, -2.0, 1e-9, -3.3e-5,
    ];
    for format in INTS
        .into_iter()
        .chain([SampleFormat::F32, SampleFormat::F64])
    {
        let (got, _) = quantise(format, Dither::None, &x);
        let input = AudioFrame {
            sample_rate: 48000,
            layout: ChannelLayout::mono(),
            pts: 0,
            samples: Samples::F64(x.to_vec()),
        };
        let mut want = input.clone();
        convert_format(&input, format, &mut want).unwrap();
        assert_eq!(got, want.samples, "{format:?}");
    }
}

#[test]
fn floats_are_never_dithered_or_clipped() {
    let x = [0.0, 0.123_456_789_012_345, 2.5, -7.0, 1e-30];
    let (f64s, c) = quantise(SampleFormat::F64, Dither::Tpdf, &x);
    assert_eq!((f64s, c), (Samples::F64(x.to_vec()), 0));
    let (f32s, c) = quantise(SampleFormat::F32, Dither::Tpdf, &x);
    let want: Vec<f32> = x.iter().map(|&v| v as f32).collect();
    assert_eq!((f32s, c), (Samples::F32(want), 0));
}

#[test]
fn tpdf_on_silence_is_two_lsb_peak_to_peak_around_zero() {
    let (v, clipped) = s16(&vec![0.0; N], Dither::Tpdf);
    assert_eq!(clipped, 0);
    let (lo, hi) = (*v.iter().min().unwrap(), *v.iter().max().unwrap());
    assert_eq!((lo, hi), (-1, 1), "peak to peak is two LSB");
    let m = mean(v.iter().map(|&s| f64::from(s)));
    assert!(m.abs() < 0.01, "mean {m} LSB");
    // Triangular density on (-1, 1) rounded: P(0) = 3/4, P(+-1) = 1/8 each.
    let zeros = v.iter().filter(|&&s| s == 0).count() as f64 / N as f64;
    assert!((zeros - 0.75).abs() < 0.01, "{zeros}");
}

#[test]
fn the_error_has_the_same_mean_and_power_whatever_the_signal() {
    // TPDF's point: the total error's first two moments do not depend on the input, so it is
    // noise, not distortion. Plain rounding fails this (its error follows the offset).
    for offset in [0.0, 0.1, 0.25, 0.5, 0.75, 0.9, -0.3] {
        let x = vec![(1000.0 + offset) * LSB; N];
        let (v, _) = s16(&x, Dither::Tpdf);
        let err = v.iter().map(|&s| f64::from(s) - (1000.0 + offset));
        let (m, var) = (mean(err.clone()), variance(err));
        assert!(m.abs() < 0.01, "offset {offset}: mean {m}");
        assert!((var - 0.25).abs() < 0.01, "offset {offset}: variance {var}");
    }
}

#[test]
fn the_error_is_uncorrelated_with_a_low_level_signal() {
    // A three-LSB sine, where undithered rounding error is strongly signal-shaped.
    let x: Vec<f64> = (0..N)
        .map(|i| 3.0 * LSB * libm::sin(i as f64 * 0.001_234))
        .collect();
    let corr = |v: &[i16]| {
        let e: Vec<f64> = v
            .iter()
            .zip(&x)
            .map(|(&s, &x)| f64::from(s) - x / LSB)
            .collect();
        let s: Vec<f64> = x.iter().map(|&x| x / LSB).collect();
        let (me, ms) = (mean(e.iter().copied()), mean(s.iter().copied()));
        let cov = mean(e.iter().zip(&s).map(|(a, b)| (a - me) * (b - ms)));
        cov / (variance(e.iter().copied()) * variance(s.iter().copied())).sqrt()
    };
    let dithered = corr(&s16(&x, Dither::Tpdf).0);
    assert!(dithered.abs() < 0.01, "dithered correlation {dithered}");
    let rounded = corr(&s16(&x, Dither::None).0);
    assert!(
        rounded.abs() > 0.05,
        "plain rounding should correlate: {rounded}"
    );
}

#[test]
fn the_same_input_gives_the_same_bytes_however_it_is_split() {
    let x: Vec<f64> = (0..10_000)
        .map(|i| (f64::from(i) * 0.37).fract() - 0.5)
        .collect();
    for format in INTS {
        let (whole, _) = quantise(format, Dither::Tpdf, &x);
        assert_eq!(quantise(format, Dither::Tpdf, &x).0, whole, "repeat");
        let mut q = Quantiser::new(format, Dither::Tpdf);
        let mut joined = Vec::new();
        for chunk in x.chunks(777) {
            let mut out = Samples::new(format);
            q.quantise(chunk, &mut out);
            joined.extend(ints(&out));
        }
        assert_eq!(joined, ints(&whole), "{format:?}");
    }
}

fn ints(s: &Samples) -> Vec<i64> {
    match s {
        Samples::U8(v) => v.iter().map(|&b| i64::from(b)).collect(),
        Samples::S16(v) => v.iter().map(|&b| i64::from(b)).collect(),
        Samples::S24(v) | Samples::S32(v) => v.iter().map(|&b| i64::from(b)).collect(),
        _ => unreachable!(),
    }
}

#[test]
fn the_generator_is_pinned() {
    // A change of generator or seed changes every dithered file; this catches it.
    let (v, _) = s16(&[0.25 * LSB; 16], Dither::Tpdf);
    assert_eq!(v, PINNED);
}

const PINNED: [i16; 16] = [0, 0, 0, 0, 1, 1, 0, 1, 1, 1, 1, 1, 0, 0, 0, 1];

#[test]
fn saturation_is_counted_and_dither_alone_never_counts() {
    let x = [
        1.5,
        -1.5,
        0.999,
        1.0,
        -1.0,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ];
    let (v, clipped) = s16(&x, Dither::None);
    assert_eq!(v, [32767, -32768, 32735, 32767, -32768, 0, 32767, -32768]);
    assert_eq!(clipped, 5, "1.5, -1.5, 1.0 and both infinities");
    // Full scale negative is representable; dither pushing it past is clamped quietly.
    let (v, clipped) = s16(&vec![-1.0; 10_000], Dither::Tpdf);
    assert_eq!(clipped, 0);
    assert!(v.iter().all(|&s| s == -32768 || s == -32767));
    assert!(v.contains(&-32767));
    let (_, clipped) = s16(&[1.5, 0.0, -2.0], Dither::Tpdf);
    assert_eq!(clipped, 2);
}

#[test]
fn eight_bit_output_is_offset_binary() {
    let (v, _) = quantise(SampleFormat::U8, Dither::None, &[0.0, -1.0, 0.5]);
    assert_eq!(v, Samples::U8(vec![0x80, 0x00, 0xC0]));
}

proptest! {
    #[test]
    fn dither_moves_a_sample_by_at_most_one_lsb(x in -1.0f64..0.999, format in 0usize..4) {
        let format = INTS[format];
        let (plain, _) = quantise(format, Dither::None, &[x; 8]);
        let (dithered, _) = quantise(format, Dither::Tpdf, &[x; 8]);
        for (a, b) in ints(&plain).into_iter().zip(ints(&dithered)) {
            prop_assert!((a - b).abs() <= 1, "{format:?} {x}: {a} vs {b}");
        }
    }
}
