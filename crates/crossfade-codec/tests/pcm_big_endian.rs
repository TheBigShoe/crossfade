//! Big-endian PCM and signed 8-bit: byte patterns, the little-endian twin, framing.

use crossfade_codec::pcm::{PcmDecoder, PcmEncoder};
use crossfade_core::{
    AudioFrame, AudioSpec, ChannelLayout, CodecId, Decoder, Encoder, Error, Packet, SampleFormat,
    Samples, StreamInfo, TimeBase,
};
use proptest::prelude::*;

/// (big-endian codec, its little-endian twin, sample format).
const PAIRS: [(CodecId, CodecId, SampleFormat); 5] = [
    (CodecId::PcmS16Be, CodecId::PcmS16Le, SampleFormat::S16),
    (CodecId::PcmS24Be, CodecId::PcmS24Le, SampleFormat::S24),
    (CodecId::PcmS32Be, CodecId::PcmS32Le, SampleFormat::S32),
    (CodecId::PcmF32Be, CodecId::PcmF32Le, SampleFormat::F32),
    (CodecId::PcmF64Be, CodecId::PcmF64Le, SampleFormat::F64),
];

fn audio(format: SampleFormat, channels: u16) -> AudioSpec {
    AudioSpec::new(
        48_000,
        format,
        ChannelLayout::unpositioned(channels).unwrap(),
    )
    .unwrap()
}

fn stream(codec: CodecId, format: SampleFormat, channels: u16) -> StreamInfo {
    StreamInfo::new(
        0,
        codec,
        audio(format, channels),
        TimeBase::new(1, 48_000).unwrap(),
    )
}

fn decode(
    codec: CodecId,
    format: SampleFormat,
    channels: u16,
    data: &[u8],
) -> Result<Samples, Error> {
    let info = stream(codec, format, channels);
    let mut dec = PcmDecoder::new(&info)?;
    dec.send_packet(&Packet {
        data: data.to_vec(),
        ..Packet::default()
    })?;
    let mut f = AudioFrame::new(&info.audio);
    assert!(dec.receive_frame(&mut f)?);
    Ok(f.samples)
}

fn encode(codec: CodecId, format: SampleFormat, channels: u16, samples: Samples) -> Vec<u8> {
    let sp = audio(format, channels);
    let mut enc = PcmEncoder::new(codec, &sp).unwrap();
    let f = AudioFrame {
        sample_rate: sp.sample_rate(),
        layout: sp.layout().clone(),
        pts: 0,
        samples,
    };
    enc.send_frame(&f).unwrap();
    let mut p = Packet::default();
    assert!(enc.receive_packet(&mut p).unwrap());
    p.data
}

#[test]
fn decodes_hand_written_bytes() {
    let one = |c, f, b: &[u8]| decode(c, f, 1, b).unwrap();
    assert_eq!(
        one(
            CodecId::PcmS16Be,
            SampleFormat::S16,
            &[0x80, 0, 0x7F, 0xFF, 0xFF, 0xFF, 0, 1]
        ),
        Samples::S16(vec![i16::MIN, i16::MAX, -1, 1])
    );
    assert_eq!(
        one(
            CodecId::PcmS24Be,
            SampleFormat::S24,
            &[0x80, 0, 0, 0x7F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 1]
        ),
        Samples::S24(vec![-8_388_608, 8_388_607, -1, 1])
    );
    assert_eq!(
        one(
            CodecId::PcmS32Be,
            SampleFormat::S32,
            &[0x80, 0, 0, 0, 0x7F, 0xFF, 0xFF, 0xFF]
        ),
        Samples::S32(vec![i32::MIN, i32::MAX])
    );
    assert_eq!(
        one(CodecId::PcmF32Be, SampleFormat::F32, &[0x3F, 0x80, 0, 0]),
        Samples::F32(vec![1.0])
    );
    assert_eq!(
        one(
            CodecId::PcmF64Be,
            SampleFormat::F64,
            &[0xBF, 0xF0, 0, 0, 0, 0, 0, 0]
        ),
        Samples::F64(vec![-1.0])
    );
}

#[test]
fn encodes_hand_written_samples() {
    let one = |c, f, s| encode(c, f, 1, s);
    assert_eq!(
        one(
            CodecId::PcmS16Be,
            SampleFormat::S16,
            Samples::S16(vec![i16::MIN, i16::MAX, -1, 0])
        ),
        [0x80, 0, 0x7F, 0xFF, 0xFF, 0xFF, 0, 0]
    );
    assert_eq!(
        one(
            CodecId::PcmS24Be,
            SampleFormat::S24,
            Samples::S24(vec![-8_388_608, 8_388_607, -1, 0])
        ),
        [0x80, 0, 0, 0x7F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0]
    );
    assert_eq!(
        one(
            CodecId::PcmS32Be,
            SampleFormat::S32,
            Samples::S32(vec![i32::MIN, i32::MAX])
        ),
        [0x80, 0, 0, 0, 0x7F, 0xFF, 0xFF, 0xFF]
    );
    assert_eq!(
        one(
            CodecId::PcmF32Be,
            SampleFormat::F32,
            Samples::F32(vec![1.0])
        ),
        [0x3F, 0x80, 0, 0]
    );
    assert_eq!(
        one(
            CodecId::PcmF64Be,
            SampleFormat::F64,
            Samples::F64(vec![-1.0])
        ),
        [0xBF, 0xF0, 0, 0, 0, 0, 0, 0]
    );
}

#[test]
fn signed_8_bit_is_offset_into_the_engine_u8() {
    // 0 is silence in signed 8-bit and 128 in the engine's unsigned format.
    assert_eq!(
        decode(
            CodecId::PcmS8,
            SampleFormat::U8,
            1,
            &[0x80, 0xFF, 0, 1, 0x7F]
        )
        .unwrap(),
        Samples::U8(vec![0, 127, 128, 129, 255])
    );
    assert_eq!(
        encode(
            CodecId::PcmS8,
            SampleFormat::U8,
            1,
            Samples::U8(vec![0, 127, 128, 129, 255])
        ),
        [0x80, 0xFF, 0, 1, 0x7F]
    );
}

#[test]
fn every_sample_value_class_round_trips() {
    let ints: [(CodecId, SampleFormat, Samples); 4] = [
        (
            CodecId::PcmS8,
            SampleFormat::U8,
            Samples::U8(vec![0, 1, 127, 128, 129, 254, 255]),
        ),
        (
            CodecId::PcmS16Be,
            SampleFormat::S16,
            Samples::S16(vec![i16::MIN, -1, 0, 1, i16::MAX]),
        ),
        (
            CodecId::PcmS24Be,
            SampleFormat::S24,
            Samples::S24(vec![-8_388_608, -1, 0, 1, 8_388_607]),
        ),
        (
            CodecId::PcmS32Be,
            SampleFormat::S32,
            Samples::S32(vec![i32::MIN, -1, 0, 1, i32::MAX]),
        ),
    ];
    let floats = [
        (
            CodecId::PcmF32Be,
            SampleFormat::F32,
            Samples::F32(vec![0.0, -0.0, 1.0, -1.0, f32::MIN_POSITIVE, f32::INFINITY]),
        ),
        (
            CodecId::PcmF64Be,
            SampleFormat::F64,
            Samples::F64(vec![
                0.0,
                -0.0,
                1.0,
                -1.0,
                f64::MIN_POSITIVE,
                f64::NEG_INFINITY,
            ]),
        ),
    ];
    for (codec, format, samples) in ints.into_iter().chain(floats) {
        let bytes = encode(codec, format, 1, samples.clone());
        assert_eq!(
            decode(codec, format, 1, &bytes).unwrap(),
            samples,
            "{codec:?}"
        );
    }
}

#[test]
fn a_packet_that_is_not_whole_frames_is_rejected() {
    for (codec, format, channels, len) in [
        (CodecId::PcmS8, SampleFormat::U8, 3, 4),
        (CodecId::PcmS16Be, SampleFormat::S16, 2, 6),
        (CodecId::PcmS24Be, SampleFormat::S24, 1, 4),
        (CodecId::PcmS32Be, SampleFormat::S32, 2, 12),
        (CodecId::PcmF32Be, SampleFormat::F32, 1, 3),
        (CodecId::PcmF64Be, SampleFormat::F64, 2, 17),
    ] {
        let info = stream(codec, format, channels);
        let mut dec = PcmDecoder::new(&info).unwrap();
        let err = dec
            .send_packet(&Packet {
                data: vec![0; len],
                ..Packet::default()
            })
            .unwrap_err();
        assert!(matches!(err, Error::Malformed { .. }), "{codec:?}: {err:?}");
    }
}

fn samples_from(format: SampleFormat, raw: &[u64]) -> Samples {
    let r = raw.iter().copied();
    match format {
        SampleFormat::U8 => Samples::U8(r.map(|x| x as u8).collect()),
        SampleFormat::S16 => Samples::S16(r.map(|x| x as i16).collect()),
        SampleFormat::S24 => Samples::S24(r.map(|x| (x as i32) << 8 >> 8).collect()),
        SampleFormat::S32 => Samples::S32(r.map(|x| x as i32).collect()),
        SampleFormat::F32 => Samples::F32(r.map(|x| f32::from_bits(x as u32)).collect()),
        SampleFormat::F64 => Samples::F64(r.map(f64::from_bits).collect()),
    }
}

proptest! {
    #[test]
    fn big_endian_is_the_little_endian_bytes_swapped(
        pair in 0..PAIRS.len(),
        channels in 1u16..=4,
        frames in 0usize..30,
        raw in prop::collection::vec(any::<u64>(), 120),
    ) {
        let (be, le, format) = PAIRS[pair];
        let samples = samples_from(format, &raw[..frames * usize::from(channels)]);
        let big = encode(be, format, channels, samples.clone());
        let little = encode(le, format, channels, samples.clone());
        let size = format.bytes_per_sample();
        let swapped: Vec<u8> = little
            .chunks_exact(size)
            .flat_map(|c| c.iter().rev().copied())
            .collect();
        prop_assert_eq!(&big, &swapped);
        // The decoders agree on the values too, bit for bit.
        let from_be = decode(be, format, channels, &big).unwrap();
        let from_le = decode(le, format, channels, &little).unwrap();
        prop_assert_eq!(encode(le, format, channels, from_be), little);
        prop_assert_eq!(encode(be, format, channels, from_le), big);
    }

    #[test]
    fn signed_8_bit_is_unsigned_8_bit_with_the_top_bit_flipped(
        bytes in prop::collection::vec(any::<u8>(), 0..64),
    ) {
        let decoded = decode(CodecId::PcmS8, SampleFormat::U8, 1, &bytes).unwrap();
        let want: Vec<u8> = bytes.iter().map(|b| b ^ 0x80).collect();
        prop_assert_eq!(&decoded, &Samples::U8(want.clone()));
        prop_assert_eq!(encode(CodecId::PcmS8, SampleFormat::U8, 1, Samples::U8(want)), bytes);
    }
}
