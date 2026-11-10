use crossfade_codec::pcm::PcmDecoder;
use crossfade_core::{
    AudioFrame, AudioSpec, ChannelLayout, CodecId, Decoder, Error, Packet, SampleFormat, Samples,
    StreamInfo, TimeBase,
};
use crossfade_testkit::{
    pcm::{decode_le, encode_le},
    signal::{Signal, render},
};

const FORMATS: [SampleFormat; 6] = [
    SampleFormat::U8,
    SampleFormat::S16,
    SampleFormat::S24,
    SampleFormat::S32,
    SampleFormat::F32,
    SampleFormat::F64,
];

fn stream(format: SampleFormat, channels: u16, rate: u32) -> StreamInfo {
    let layout = if channels == 2 {
        ChannelLayout::stereo()
    } else {
        ChannelLayout::unpositioned(channels).unwrap()
    };
    StreamInfo {
        index: 0,
        codec: CodecId::pcm_for(format),
        audio: AudioSpec::new(rate, format, layout).unwrap(),
        time_base: TimeBase::new(1, i64::from(rate)).unwrap(),
        frames: None,
    }
}

fn packet(pts: i64, data: &[u8]) -> Packet {
    Packet {
        stream: 0,
        pts,
        duration: 0,
        data: data.to_vec(),
    }
}

fn decode(info: &StreamInfo, data: &[u8]) -> Result<AudioFrame, Error> {
    let mut dec = PcmDecoder::new(info).unwrap();
    dec.send_packet(&packet(0, data))?;
    let mut frame = AudioFrame::new(&info.audio);
    assert!(dec.receive_frame(&mut frame)?);
    Ok(frame)
}

/// Samples as the oracle scales them, so equality is exact.
fn as_f64(samples: &Samples) -> Vec<f64> {
    match samples {
        Samples::U8(v) => v.iter().map(|&x| (f64::from(x) - 128.0) / 128.0).collect(),
        Samples::S16(v) => v.iter().map(|&x| f64::from(x) / 32768.0).collect(),
        Samples::S24(v) => v.iter().map(|&x| f64::from(x) / 8_388_608.0).collect(),
        Samples::S32(v) => v.iter().map(|&x| f64::from(x) / 2_147_483_648.0).collect(),
        Samples::F32(v) => v.iter().map(|&x| f64::from(x)).collect(),
        Samples::F64(v) => v.clone(),
    }
}

#[test]
fn decodes_identically_to_the_oracle_for_every_format() {
    for format in FORMATS {
        let info = stream(format, 2, 48_000);
        let signal = Signal::Noise {
            seed: 7,
            amplitude: 0.9,
        };
        let samples = render(&signal, &info.audio, 500).unwrap();
        let bytes = encode_le(&samples, format);
        let frame = decode(&info, &bytes).unwrap();
        assert_eq!(frame.samples.format(), format);
        assert_eq!(
            as_f64(&frame.samples),
            decode_le(&bytes, format).unwrap(),
            "{format:?}"
        );
        assert_eq!(frame.frames(), 500);
    }
}

#[test]
fn known_byte_patterns() {
    let one = |f, b: &[u8]| decode(&stream(f, 1, 8000), b).unwrap().samples;
    assert_eq!(
        one(SampleFormat::U8, &[0, 128, 255]),
        Samples::U8(vec![0, 128, 255])
    );
    assert_eq!(
        one(SampleFormat::S16, &[0, 0x80, 0xFF, 0x7F, 1, 0]),
        Samples::S16(vec![i16::MIN, i16::MAX, 1])
    );
    assert_eq!(
        one(
            SampleFormat::S24,
            &[0, 0, 0x80, 0xFF, 0xFF, 0x7F, 0xFF, 0xFF, 0xFF, 1, 0, 0]
        ),
        Samples::S24(vec![-8_388_608, 8_388_607, -1, 1])
    );
    assert_eq!(
        one(SampleFormat::S32, &[0, 0, 0, 0x80, 0xFF, 0xFF, 0xFF, 0x7F]),
        Samples::S32(vec![i32::MIN, i32::MAX])
    );
    assert_eq!(
        one(SampleFormat::F32, &[0, 0, 0x80, 0x3F]),
        Samples::F32(vec![1.0])
    );
    assert_eq!(
        one(SampleFormat::F64, &[0, 0, 0, 0, 0, 0, 0xF0, 0xBF]),
        Samples::F64(vec![-1.0])
    );
}

#[test]
fn frame_carries_stream_rate_layout_and_packet_pts() {
    let info = stream(SampleFormat::S16, 2, 44_100);
    let mut dec = PcmDecoder::new(&info).unwrap();
    dec.send_packet(&packet(1234, &[0; 8])).unwrap();
    let mut frame = AudioFrame::new(&info.audio);
    assert!(dec.receive_frame(&mut frame).unwrap());
    assert_eq!(frame.sample_rate, 44_100);
    assert_eq!(frame.layout, ChannelLayout::stereo());
    assert_eq!(frame.pts, 1234);
}

#[test]
fn partial_frame_is_malformed() {
    // Stereo S16 frames are 4 bytes; S24 mono frames are 3.
    for (format, channels, len) in [
        (SampleFormat::S16, 2, 6),
        (SampleFormat::S24, 1, 4),
        (SampleFormat::F64, 2, 17),
        (SampleFormat::U8, 3, 4),
    ] {
        let info = stream(format, channels, 8000);
        let mut dec = PcmDecoder::new(&info).unwrap();
        let err = dec.send_packet(&packet(0, &vec![0; len])).unwrap_err();
        assert!(
            matches!(err, Error::Malformed { .. }),
            "{format:?}: {err:?}"
        );
    }
}

#[test]
fn empty_packet_gives_empty_frame() {
    let info = stream(SampleFormat::S16, 2, 8000);
    let frame = decode(&info, &[]).unwrap();
    assert_eq!(frame.frames(), 0);
    assert!(frame.samples.is_empty());
}

#[test]
fn output_format_follows_the_codec_not_the_spec() {
    // The codec decides the sample format; a disagreeing spec format is ignored.
    let mut info = stream(SampleFormat::S16, 2, 8000);
    info.audio = AudioSpec::new(8000, SampleFormat::F32, ChannelLayout::stereo()).unwrap();
    let frame = decode(&info, &[0; 4]).unwrap();
    assert_eq!(frame.samples.format(), SampleFormat::S16);
    assert_eq!(frame.frames(), 1);
}

#[test]
fn receive_hands_over_once_then_reports_nothing() {
    let info = stream(SampleFormat::S16, 1, 8000);
    let mut dec = PcmDecoder::new(&info).unwrap();
    let mut frame = AudioFrame::new(&info.audio);
    assert!(!dec.receive_frame(&mut frame).unwrap());
    dec.send_packet(&packet(0, &[1, 0, 2, 0])).unwrap();
    assert!(dec.receive_frame(&mut frame).unwrap());
    assert!(!dec.receive_frame(&mut frame).unwrap());
    // The decoder is reusable after its output was taken.
    dec.send_packet(&packet(2, &[3, 0])).unwrap();
    assert!(dec.receive_frame(&mut frame).unwrap());
    assert_eq!(frame.samples, Samples::S16(vec![3]));
    assert_eq!(frame.pts, 2);
}

#[test]
fn second_send_before_receive_is_unsupported() {
    let info = stream(SampleFormat::S16, 1, 8000);
    let mut dec = PcmDecoder::new(&info).unwrap();
    dec.send_packet(&packet(0, &[1, 0])).unwrap();
    let err = dec.send_packet(&packet(1, &[2, 0])).unwrap_err();
    assert!(matches!(err, Error::Unsupported { .. }), "{err:?}");
    // The pending output survives the refused send.
    let mut frame = AudioFrame::new(&info.audio);
    assert!(dec.receive_frame(&mut frame).unwrap());
    assert_eq!(frame.samples, Samples::S16(vec![1]));
}

#[test]
fn drain_returns_pending_output_then_nothing() {
    let info = stream(SampleFormat::S16, 1, 8000);
    let mut dec = PcmDecoder::new(&info).unwrap();
    let mut frame = AudioFrame::new(&info.audio);
    dec.send_packet(&packet(0, &[1, 0])).unwrap();
    dec.drain();
    assert!(dec.receive_frame(&mut frame).unwrap());
    assert!(!dec.receive_frame(&mut frame).unwrap());
    assert!(!dec.receive_frame(&mut frame).unwrap());
}

#[test]
fn output_buffer_is_swapped_not_copied() {
    let info = stream(SampleFormat::S16, 1, 8000);
    let mut dec = PcmDecoder::new(&info).unwrap();
    let mut frame = AudioFrame::new(&info.audio);
    dec.send_packet(&packet(0, &[0; 64])).unwrap();
    assert!(dec.receive_frame(&mut frame).unwrap());
    let ptr = match &frame.samples {
        Samples::S16(v) => v.as_ptr(),
        _ => unreachable!(),
    };
    // Second round: the decoder now owns the first buffer and refills it.
    dec.send_packet(&packet(0, &[0; 64])).unwrap();
    assert!(dec.receive_frame(&mut frame).unwrap());
    dec.send_packet(&packet(0, &[0; 64])).unwrap();
    assert!(dec.receive_frame(&mut frame).unwrap());
    let again = match &frame.samples {
        Samples::S16(v) => v.as_ptr(),
        _ => unreachable!(),
    };
    assert_eq!(ptr, again, "two buffers ping-pong, nothing reallocates");
}
