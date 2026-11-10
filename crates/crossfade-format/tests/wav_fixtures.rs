//! Multichannel float files written by Apple's `afconvert` (see fixtures/wav/README.md) read
//! back with their channel positions and exactly the samples they were converted from.

use crossfade_codec::pcm::PcmDecoder;
use crossfade_core::{
    AudioFrame, Channel, ChannelLayout, Decoder, Demuxer, Limits, Packet, SampleFormat, Samples,
};
use crossfade_format::wav::WavDemuxer;
use std::io::Cursor;

const FRAMES: usize = 32;

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/wav/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(path).unwrap()
}

/// The 16-bit source (make_source.py) as floats: afconvert divides by 32768, which is exact.
fn expected(channels: usize) -> Vec<f64> {
    (0..FRAMES)
        .flat_map(|f| (0..channels).map(move |c| (f * 37 + c * 1000) as f64 - 3000.0))
        .map(|v| v / 32768.0)
        .collect()
}

fn decode(name: &str) -> (ChannelLayout, SampleFormat, Vec<f64>) {
    let mut d = WavDemuxer::new(Box::new(Cursor::new(fixture(name))), &Limits::default()).unwrap();
    let info = d.streams()[0].clone();
    assert_eq!(info.frames, Some(FRAMES as u64), "{name}");
    let mut dec = PcmDecoder::new(&info).unwrap();
    let mut frame = AudioFrame::new(&info.audio);
    let mut pkt = Packet::default();
    let mut out = Vec::new();
    while d.read_packet(&mut pkt).unwrap() {
        dec.send_packet(&pkt).unwrap();
        assert!(dec.receive_frame(&mut frame).unwrap());
        match &frame.samples {
            Samples::F32(v) => out.extend(v.iter().map(|&x| f64::from(x))),
            Samples::F64(v) => out.extend(v),
            other => panic!("{name}: not float: {other:?}"),
        }
    }
    (info.audio.layout().clone(), info.audio.format(), out)
}

#[test]
fn afconvert_multichannel_float_keeps_positions_and_samples() {
    use Channel::*;
    let five_one = vec![FrontLeft, FrontRight, FrontCenter, Lfe, SideLeft, SideRight];
    let quad = vec![FrontLeft, FrontRight, BackLeft, BackRight];
    let cases = [
        ("f32-5.1.wav", SampleFormat::F32, five_one.clone()),
        ("f64-5.1.wav", SampleFormat::F64, five_one),
        ("f32-quad.wav", SampleFormat::F32, quad),
    ];
    for (name, format, positions) in cases {
        let (layout, got_format, samples) = decode(name);
        let channels = positions.len();
        assert_eq!(layout, ChannelLayout::from_positions(positions).unwrap());
        assert_eq!(got_format, format, "{name}");
        assert_eq!(samples, expected(channels), "{name}");
    }
}
