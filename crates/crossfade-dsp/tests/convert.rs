use crossfade_core::{AudioFrame, ChannelLayout, SampleFormat, Samples};
use crossfade_dsp::convert::convert_format;
use crossfade_testkit::pcm::{decode_le, encode_le};
use proptest::prelude::*;

const INTS: [SampleFormat; 4] = [
    SampleFormat::U8,
    SampleFormat::S16,
    SampleFormat::S24,
    SampleFormat::S32,
];
const ALL: [SampleFormat; 6] = [
    SampleFormat::U8,
    SampleFormat::S16,
    SampleFormat::S24,
    SampleFormat::S32,
    SampleFormat::F32,
    SampleFormat::F64,
];

fn frame(samples: Samples) -> AudioFrame {
    AudioFrame {
        sample_rate: 48_000,
        layout: ChannelLayout::mono(),
        pts: 0,
        samples,
    }
}

fn empty_out() -> AudioFrame {
    frame(Samples::U8(vec![]))
}

fn convert(input: &AudioFrame, to: SampleFormat) -> Samples {
    let mut out = empty_out();
    convert_format(input, to, &mut out).unwrap();
    assert_eq!(out.samples.format(), to);
    out.samples
}

/// Signed value of an integer sample on its own scale (u8 offset removed).
fn ints(s: &Samples) -> Vec<i64> {
    match s {
        Samples::U8(v) => v.iter().map(|&x| i64::from(x) - 128).collect(),
        Samples::S16(v) => v.iter().map(|&x| i64::from(x)).collect(),
        Samples::S24(v) | Samples::S32(v) => v.iter().map(|&x| i64::from(x)).collect(),
        _ => panic!("not an integer format"),
    }
}

/// Builds a frame from signed values already inside the format's range.
fn from_ints(format: SampleFormat, v: &[i64]) -> Samples {
    match format {
        SampleFormat::U8 => Samples::U8(v.iter().map(|&x| (x + 128) as u8).collect()),
        SampleFormat::S16 => Samples::S16(v.iter().map(|&x| x as i16).collect()),
        SampleFormat::S24 => Samples::S24(v.iter().map(|&x| x as i32).collect()),
        SampleFormat::S32 => Samples::S32(v.iter().map(|&x| x as i32).collect()),
        _ => panic!("not an integer format"),
    }
}

/// Wraps a raw value into the format's signed range.
fn wrap(format: SampleFormat, x: i64) -> i64 {
    let shift = 64 - format.bits();
    x << shift >> shift
}

#[test]
fn same_format_copies() {
    for f in INTS {
        let input = frame(from_ints(f, &[1, -2, 0, -128, 127]));
        assert_eq!(convert(&input, f), input.samples, "{f:?}");
    }
    let input = frame(Samples::F32(vec![0.25, -3.5]));
    assert_eq!(convert(&input, SampleFormat::F32), input.samples);
    let input = frame(Samples::F64(vec![0.25, -3.5]));
    assert_eq!(convert(&input, SampleFormat::F64), input.samples);
}

#[test]
fn widening_shifts_left() {
    let s16 = frame(Samples::S16(vec![1, -1, i16::MAX, i16::MIN]));
    assert_eq!(
        convert(&s16, SampleFormat::S24),
        Samples::S24(vec![256, -256, 0x7FFF00, -0x800000])
    );
    assert_eq!(
        convert(&s16, SampleFormat::S32),
        Samples::S32(vec![65536, -65536, 0x7FFF_0000, i32::MIN])
    );
    let s24 = frame(Samples::S24(vec![1, -1, 0x7F_FFFF, -0x80_0000]));
    assert_eq!(
        convert(&s24, SampleFormat::S32),
        Samples::S32(vec![256, -256, 0x7FFF_FF00, i32::MIN])
    );
}

#[test]
fn narrowing_is_an_arithmetic_shift() {
    let s32 = frame(Samples::S32(vec![
        0x1234_5678,
        -1,
        -65536,
        -65535,
        i32::MAX,
        i32::MIN,
    ]));
    assert_eq!(
        convert(&s32, SampleFormat::S16),
        Samples::S16(vec![0x1234, -1, -1, -1, i16::MAX, i16::MIN])
    );
    assert_eq!(
        convert(&s32, SampleFormat::S24),
        Samples::S24(vec![0x12_3456, -1, -256, -256, 0x7F_FFFF, -0x80_0000])
    );
    let s16 = frame(Samples::S16(vec![-1, 255, 256, i16::MIN, i16::MAX]));
    // -1 >> 8 stays -1, which is 127 in offset binary.
    assert_eq!(
        convert(&s16, SampleFormat::U8),
        Samples::U8(vec![127, 128, 129, 0, 255])
    );
}

#[test]
fn u8_is_offset_binary() {
    let u = frame(Samples::U8(vec![0, 128, 255]));
    assert_eq!(
        convert(&u, SampleFormat::S16),
        Samples::S16(vec![-32768, 0, 127 * 256])
    );
    assert_eq!(
        convert(&u, SampleFormat::S32),
        Samples::S32(vec![i32::MIN, 0, 127 << 24])
    );
    assert_eq!(
        convert(&u, SampleFormat::F32),
        Samples::F32(vec![-1.0, 0.0, 127.0 / 128.0])
    );
    assert_eq!(
        convert(&u, SampleFormat::F64),
        Samples::F64(vec![-1.0, 0.0, 127.0 / 128.0])
    );
    let f = frame(Samples::F64(vec![-1.0, 0.0, 1.0]));
    assert_eq!(
        convert(&f, SampleFormat::U8),
        Samples::U8(vec![0, 128, 255])
    );
}

#[test]
fn int_to_float_divides_by_two_to_the_bits_minus_one() {
    let s16 = frame(Samples::S16(vec![i16::MIN, 16384, i16::MAX]));
    assert_eq!(
        convert(&s16, SampleFormat::F32),
        Samples::F32(vec![-1.0, 0.5, 32767.0 / 32768.0])
    );
    let s32 = frame(Samples::S32(vec![i32::MIN, 1 << 30, i32::MAX]));
    assert_eq!(
        convert(&s32, SampleFormat::F64),
        Samples::F64(vec![-1.0, 0.5, 2_147_483_647.0 / 2_147_483_648.0])
    );
}

#[test]
fn float_to_float_is_a_plain_cast() {
    let f64s = frame(Samples::F64(vec![0.1, -1e300, f64::INFINITY]));
    assert_eq!(
        convert(&f64s, SampleFormat::F32),
        Samples::F32(vec![0.1f64 as f32, f32::NEG_INFINITY, f32::INFINITY])
    );
    let f32s = frame(Samples::F32(vec![0.1, -2.5]));
    assert_eq!(
        convert(&f32s, SampleFormat::F64),
        Samples::F64(vec![f64::from(0.1f32), -2.5])
    );
}

#[test]
fn float_to_int_matches_the_oracle_on_edge_values() {
    let mut edges = vec![
        0.0,
        -0.0,
        1.0,
        -1.0,
        1.0 + 1e-9,
        -1.0 - 1e-9,
        2.0,
        -2.0,
        1e30,
    ];
    edges.extend([f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.999_999_9]);
    for to in INTS {
        // Ties, and just either side of them.
        let lsb = 1.0 / (1u64 << (to.bits() - 1)) as f64;
        for k in [0.5, 1.5, 2.5, 0.49, 0.51] {
            edges.push(k * lsb);
            edges.push(-k * lsb);
        }
    }
    for to in INTS {
        let f64_in = frame(Samples::F64(edges.clone()));
        let want = decode_ints(&encode_le(&edges, to), to);
        assert_eq!(ints(&convert(&f64_in, to)), want, "f64 -> {to:?}");
        let f32_vals: Vec<f32> = edges.iter().map(|&x| x as f32).collect();
        let as_f64: Vec<f64> = f32_vals.iter().map(|&x| f64::from(x)).collect();
        let want = decode_ints(&encode_le(&as_f64, to), to);
        let f32_in = frame(Samples::F32(f32_vals));
        assert_eq!(ints(&convert(&f32_in, to)), want, "f32 -> {to:?}");
    }
}

/// Oracle bytes back to signed integers, independent of the converter.
fn decode_ints(bytes: &[u8], format: SampleFormat) -> Vec<i64> {
    let scale = (1u64 << (format.bits() - 1)) as f64;
    decode_le(bytes, format)
        .unwrap()
        .iter()
        .map(|&x| (x * scale) as i64)
        .collect()
}

#[test]
fn rate_layout_and_pts_are_carried_over() {
    let mut input = frame(Samples::S16(vec![1, 2, 3, 4]));
    input.sample_rate = 44_100;
    input.layout = ChannelLayout::stereo();
    input.pts = -77;
    let mut out = empty_out();
    convert_format(&input, SampleFormat::F32, &mut out).unwrap();
    assert_eq!(out.sample_rate, 44_100);
    assert_eq!(out.layout, ChannelLayout::stereo());
    assert_eq!(out.pts, -77);
    assert_eq!(out.frames(), 2);
}

#[test]
fn reusing_out_leaves_no_stale_samples_and_keeps_capacity() {
    let big = frame(Samples::S16((0..1000).collect()));
    let small = frame(Samples::S16(vec![5, -6]));
    for to in ALL {
        let mut reused = empty_out();
        convert_format(&big, to, &mut reused).unwrap();
        convert_format(&small, to, &mut reused).unwrap();
        let mut fresh = empty_out();
        convert_format(&small, to, &mut fresh).unwrap();
        assert_eq!(reused, fresh, "{to:?}");
    }
    let mut out = empty_out();
    convert_format(&big, SampleFormat::S32, &mut out).unwrap();
    let Samples::S32(v) = &out.samples else {
        panic!()
    };
    let (ptr, cap) = (v.as_ptr(), v.capacity());
    convert_format(&small, SampleFormat::S32, &mut out).unwrap();
    let Samples::S32(v) = &out.samples else {
        panic!()
    };
    assert_eq!((v.as_ptr(), v.capacity()), (ptr, cap));
}

#[test]
fn empty_input_gives_empty_output() {
    for from in ALL {
        for to in ALL {
            let input = frame(Samples::new(from));
            assert!(convert(&input, to).is_empty(), "{from:?} -> {to:?}");
        }
    }
}

fn rank(f: SampleFormat) -> usize {
    f.bits()
}

proptest! {
    #[test]
    fn widening_then_narrowing_is_identity(
        a in 0..INTS.len(),
        b in 0..INTS.len(),
        raw in prop::collection::vec(any::<i64>(), 0..64),
    ) {
        let (from, wide) = (INTS[a], INTS[b]);
        prop_assume!(rank(wide) >= rank(from));
        let values: Vec<i64> = raw.iter().map(|&x| wrap(from, x)).collect();
        let input = frame(from_ints(from, &values));
        let up = frame(convert(&input, wide));
        prop_assert_eq!(convert(&up, from), input.samples);
    }

    #[test]
    fn integers_survive_f64(a in 0..INTS.len(), raw in prop::collection::vec(any::<i64>(), 0..64)) {
        let from = INTS[a];
        let values: Vec<i64> = raw.iter().map(|&x| wrap(from, x)).collect();
        let input = frame(from_ints(from, &values));
        let mid = frame(convert(&input, SampleFormat::F64));
        prop_assert_eq!(convert(&mid, from), input.samples);
    }

    #[test]
    fn narrow_integers_survive_f32(
        a in 0..3usize,
        raw in prop::collection::vec(any::<i64>(), 0..64),
    ) {
        let from = INTS[a]; // u8, s16, s24: at most 24 significant bits
        let values: Vec<i64> = raw.iter().map(|&x| wrap(from, x)).collect();
        let input = frame(from_ints(from, &values));
        let mid = frame(convert(&input, SampleFormat::F32));
        prop_assert_eq!(convert(&mid, from), input.samples);
    }

    #[test]
    fn float_to_int_agrees_with_the_oracle(
        t in 0..INTS.len(),
        xs in prop::collection::vec(-2.0f64..2.0, 0..64),
        big in prop::collection::vec(any::<f64>(), 0..8),
    ) {
        let to = INTS[t];
        let mut xs = xs;
        xs.extend(big);
        let f64_in = frame(Samples::F64(xs.clone()));
        prop_assert_eq!(ints(&convert(&f64_in, to)), decode_ints(&encode_le(&xs, to), to));
        let f32s: Vec<f32> = xs.iter().map(|&x| x as f32).collect();
        let widened: Vec<f64> = f32s.iter().map(|&x| f64::from(x)).collect();
        let f32_in = frame(Samples::F32(f32s));
        prop_assert_eq!(ints(&convert(&f32_in, to)), decode_ints(&encode_le(&widened, to), to));
    }
}
