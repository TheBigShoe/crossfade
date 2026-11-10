use crossfade_codec::pcm::{PcmDecoder, PcmEncoder};
use crossfade_core::{
    AudioFrame, AudioSpec, ChannelLayout, CodecId, Decoder, Encoder, Error, Packet, SampleFormat,
    Samples, StreamInfo, TimeBase,
};
use proptest::prelude::*;

const FORMATS: [SampleFormat; 6] = [
    SampleFormat::U8,
    SampleFormat::S16,
    SampleFormat::S24,
    SampleFormat::S32,
    SampleFormat::F32,
    SampleFormat::F64,
];

fn layout(channels: u16) -> ChannelLayout {
    ChannelLayout::unpositioned(channels).unwrap()
}

fn spec(format: SampleFormat, channels: u16) -> AudioSpec {
    AudioSpec::new(48_000, format, layout(channels)).unwrap()
}

fn stream(format: SampleFormat, channels: u16) -> StreamInfo {
    StreamInfo::new(
        0,
        CodecId::pcm_for(format),
        spec(format, channels),
        TimeBase::new(1, 48_000).unwrap(),
    )
}

fn frame(spec: &AudioSpec, pts: i64, samples: Samples) -> AudioFrame {
    AudioFrame {
        sample_rate: spec.sample_rate(),
        layout: spec.layout().clone(),
        pts,
        samples,
    }
}

fn encode(format: SampleFormat, channels: u16, samples: Samples) -> Result<Packet, Error> {
    let sp = spec(format, channels);
    let mut enc = PcmEncoder::new(CodecId::pcm_for(format), &sp).unwrap();
    enc.send_frame(&frame(&sp, 0, samples))?;
    let mut pkt = Packet::default();
    assert!(enc.receive_packet(&mut pkt)?);
    Ok(pkt)
}

#[test]
fn known_byte_patterns() {
    let one = |f, s| encode(f, 1, s).unwrap().data;
    assert_eq!(
        one(SampleFormat::U8, Samples::U8(vec![0, 128, 255])),
        [0, 128, 255]
    );
    assert_eq!(
        one(SampleFormat::S16, Samples::S16(vec![i16::MIN, i16::MAX, 1])),
        [0, 0x80, 0xFF, 0x7F, 1, 0]
    );
    assert_eq!(
        one(
            SampleFormat::S24,
            Samples::S24(vec![-8_388_608, 8_388_607, -1, 1])
        ),
        [0, 0, 0x80, 0xFF, 0xFF, 0x7F, 0xFF, 0xFF, 0xFF, 1, 0, 0]
    );
    assert_eq!(
        one(SampleFormat::S32, Samples::S32(vec![i32::MIN, i32::MAX])),
        [0, 0, 0, 0x80, 0xFF, 0xFF, 0xFF, 0x7F]
    );
    assert_eq!(
        one(SampleFormat::F32, Samples::F32(vec![1.0])),
        [0, 0, 0x80, 0x3F]
    );
    assert_eq!(
        one(SampleFormat::F64, Samples::F64(vec![-1.0])),
        [0, 0, 0, 0, 0, 0, 0xF0, 0xBF]
    );
}

#[test]
fn packet_carries_pts_duration_and_stream_zero() {
    let sp = spec(SampleFormat::S16, 2);
    let mut enc = PcmEncoder::new(CodecId::PcmS16Le, &sp).unwrap();
    enc.send_frame(&frame(&sp, 99, Samples::S16(vec![0; 10])))
        .unwrap();
    let mut pkt = Packet {
        stream: 7,
        pts: -1,
        duration: 1,
        data: vec![1],
    };
    assert!(enc.receive_packet(&mut pkt).unwrap());
    assert_eq!(
        (pkt.stream, pkt.pts, pkt.duration, pkt.data.len()),
        (0, 99, 5, 20)
    );
}

#[test]
fn wrong_sample_format_asks_for_a_conversion() {
    let sp = spec(SampleFormat::S16, 1);
    let mut enc = PcmEncoder::new(CodecId::PcmS16Le, &sp).unwrap();
    let f = frame(&sp, 0, Samples::F32(vec![0.0]));
    match enc.send_frame(&f).unwrap_err() {
        Error::Unsupported { what } => assert!(what.contains("convert"), "{what}"),
        e => panic!("{e:?}"),
    }
}

#[test]
fn mismatched_rate_or_channels_is_unsupported() {
    let sp = spec(SampleFormat::S16, 2);
    let mut enc = PcmEncoder::new(CodecId::PcmS16Le, &sp).unwrap();
    let mut f = frame(&sp, 0, Samples::S16(vec![0; 4]));
    f.sample_rate = 44_100;
    assert!(matches!(enc.send_frame(&f), Err(Error::Unsupported { .. })));
    let mut f = frame(&sp, 0, Samples::S16(vec![0; 3]));
    f.layout = layout(3);
    assert!(matches!(enc.send_frame(&f), Err(Error::Unsupported { .. })));
}

#[test]
fn samples_not_a_whole_number_of_frames_are_malformed() {
    let sp = spec(SampleFormat::S16, 2);
    let mut enc = PcmEncoder::new(CodecId::PcmS16Le, &sp).unwrap();
    let f = frame(&sp, 0, Samples::S16(vec![0; 3]));
    assert!(matches!(enc.send_frame(&f), Err(Error::Malformed { .. })));
}

#[test]
fn empty_frame_gives_empty_packet() {
    let pkt = encode(SampleFormat::S24, 2, Samples::S24(vec![])).unwrap();
    assert!(pkt.data.is_empty());
    assert_eq!(pkt.duration, 0);
}

#[test]
fn protocol_mirrors_the_decoder() {
    let sp = spec(SampleFormat::S16, 1);
    let mut enc = PcmEncoder::new(CodecId::PcmS16Le, &sp).unwrap();
    let mut pkt = Packet::default();
    assert!(!enc.receive_packet(&mut pkt).unwrap());
    enc.send_frame(&frame(&sp, 0, Samples::S16(vec![1])))
        .unwrap();
    let err = enc
        .send_frame(&frame(&sp, 1, Samples::S16(vec![2])))
        .unwrap_err();
    assert!(matches!(err, Error::Unsupported { .. }), "{err:?}");
    enc.drain();
    assert!(enc.receive_packet(&mut pkt).unwrap());
    assert_eq!(pkt.data, [1, 0]);
    assert!(!enc.receive_packet(&mut pkt).unwrap());
    assert!(!enc.receive_packet(&mut pkt).unwrap());
}

#[test]
fn a_caller_packet_buffer_is_reused_in_steady_state() {
    let sp = spec(SampleFormat::S16, 1);
    let mut enc = PcmEncoder::new(CodecId::PcmS16Le, &sp).unwrap();
    let mut pkt = Packet::default();
    let mut ptrs = Vec::new();
    for _ in 0..4 {
        enc.send_frame(&frame(&sp, 0, Samples::S16(vec![0; 32])))
            .unwrap();
        assert!(enc.receive_packet(&mut pkt).unwrap());
        ptrs.push(pkt.data.as_ptr());
    }
    assert_eq!(ptrs[1], ptrs[3], "two buffers ping-pong");
}

#[test]
fn the_spec_format_is_ignored_in_favour_of_the_codec() {
    // The spec's own format is irrelevant: the codec decides.
    let sp = spec(SampleFormat::F32, 1);
    assert!(PcmEncoder::new(CodecId::PcmS16Le, &sp).is_ok());
}

/// Raw bit patterns, so NaN payloads and subnormals round-trip too.
fn samples_from(format: SampleFormat, raw: &[u64]) -> Samples {
    let r = raw.iter().copied();
    match format {
        SampleFormat::U8 => Samples::U8(r.map(|x| x as u8).collect()),
        SampleFormat::S16 => Samples::S16(r.map(|x| x as i16).collect()),
        // Within +-2^23, the only range S24 promises.
        SampleFormat::S24 => Samples::S24(r.map(|x| (x as i32) << 8 >> 8).collect()),
        SampleFormat::S32 => Samples::S32(r.map(|x| x as i32).collect()),
        SampleFormat::F32 => Samples::F32(r.map(|x| f32::from_bits(x as u32)).collect()),
        SampleFormat::F64 => Samples::F64(r.map(f64::from_bits).collect()),
    }
}

fn bits(s: &Samples) -> Vec<u64> {
    match s {
        Samples::U8(v) => v.iter().map(|&x| u64::from(x)).collect(),
        Samples::S16(v) => v.iter().map(|&x| x as u64).collect(),
        Samples::S24(v) | Samples::S32(v) => v.iter().map(|&x| x as u64).collect(),
        Samples::F32(v) => v.iter().map(|&x| u64::from(x.to_bits())).collect(),
        Samples::F64(v) => v.iter().map(|&x| x.to_bits()).collect(),
    }
}

proptest! {
    #[test]
    fn decode_of_encode_is_identity(
        fi in 0..FORMATS.len(),
        channels in 1u16..=8,
        frames in 0usize..40,
        raw in prop::collection::vec(any::<u64>(), 320),
    ) {
        let format = FORMATS[fi];
        let n = frames * usize::from(channels);
        let samples = samples_from(format, &raw[..n]);
        let pkt = encode(format, channels, samples.clone()).unwrap();
        prop_assert_eq!(pkt.data.len(), n * format.bytes_per_sample());
        let info = stream(format, channels);
        let mut dec = PcmDecoder::new(&info).unwrap();
        dec.send_packet(&pkt).unwrap();
        let mut out = AudioFrame::new(&info.audio);
        prop_assert!(dec.receive_frame(&mut out).unwrap());
        prop_assert_eq!(bits(&out.samples), bits(&samples));
    }

    #[test]
    fn encode_of_decode_is_identity(
        fi in 0..FORMATS.len(),
        channels in 1u16..=8,
        raw in prop::collection::vec(any::<u8>(), 0..400),
    ) {
        let format = FORMATS[fi];
        let info = stream(format, channels);
        let frame_len = info.audio.bytes_per_frame();
        let bytes = &raw[..raw.len() / frame_len * frame_len];
        let mut dec = PcmDecoder::new(&info).unwrap();
        dec.send_packet(&Packet { stream: 0, pts: 0, duration: 0, data: bytes.to_vec() }).unwrap();
        let mut f = AudioFrame::new(&info.audio);
        prop_assert!(dec.receive_frame(&mut f).unwrap());
        let mut enc = PcmEncoder::new(info.codec, &info.audio).unwrap();
        enc.send_frame(&f).unwrap();
        let mut pkt = Packet::default();
        prop_assert!(enc.receive_packet(&mut pkt).unwrap());
        prop_assert_eq!(pkt.data, bytes);
    }
}
