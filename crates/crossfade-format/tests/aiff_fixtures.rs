//! Files written by Apple's `afconvert` (see fixtures/aiff/README.md) decode bit-exactly to the
//! signal they were made from. They also carry an `FLLR` filler chunk the reader must skip.

#[path = "support/signal.rs"]
mod signal;

use crossfade_codec::pcm::PcmDecoder;
use crossfade_core::{
    AudioFrame, ChannelLayout, CodecId, Decoder, Demuxer, Limits, MediaSource, Packet,
    SampleFormat, Samples, StreamInfo,
};
use crossfade_format::aiff;
use crossfade_testkit::io::FaultySource;
use std::io::Cursor;

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/aiff/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(path).unwrap()
}

fn decode_all(info: &StreamInfo, d: &mut dyn Demuxer) -> Samples {
    let mut dec = PcmDecoder::new(info).unwrap();
    let mut frame = AudioFrame::new(&info.audio);
    let mut pkt = Packet::default();
    let mut all = Samples::new(info.audio.format());
    while d.read_packet(&mut pkt).unwrap() {
        dec.send_packet(&pkt).unwrap();
        assert!(dec.receive_frame(&mut frame).unwrap());
        append(&mut all, &frame.samples);
    }
    all
}

fn append(into: &mut Samples, from: &Samples) {
    match (into, from) {
        (Samples::U8(a), Samples::U8(b)) => a.extend(b),
        (Samples::S16(a), Samples::S16(b)) => a.extend(b),
        (Samples::S24(a), Samples::S24(b)) | (Samples::S32(a), Samples::S32(b)) => a.extend(b),
        (Samples::F32(a), Samples::F32(b)) => a.extend(b),
        (Samples::F64(a), Samples::F64(b)) => a.extend(b),
        _ => panic!("format changed mid-stream"),
    }
}

/// The signal as the engine's samples, decoded from the little-endian bytes it was made from.
fn expected(format: SampleFormat, info: &StreamInfo) -> Samples {
    let le = StreamInfo {
        codec: CodecId::pcm_for(format),
        ..info.clone()
    };
    let mut dec = PcmDecoder::new(&le).unwrap();
    let mut frame = AudioFrame::new(&le.audio);
    dec.send_packet(&Packet {
        data: signal::le_bytes(format),
        ..Packet::default()
    })
    .unwrap();
    assert!(dec.receive_frame(&mut frame).unwrap());
    frame.samples
}

#[test]
fn afconvert_files_decode_to_the_signal_they_were_made_from() {
    use CodecId::*;
    use SampleFormat as F;
    // (file, codec the reader should pick, engine format)
    let cases = [
        ("s8.aiff", PcmS8, F::U8),
        ("s8-none.aifc", PcmS8, F::U8),
        ("u8-raw.aifc", PcmU8, F::U8),
        ("s16.aiff", PcmS16Be, F::S16),
        ("s16-twos.aifc", PcmS16Be, F::S16),
        ("s24.aiff", PcmS24Be, F::S24),
        ("s24-in24.aifc", PcmS24Be, F::S24),
        ("s32.aiff", PcmS32Be, F::S32),
        ("s32-in32.aifc", PcmS32Be, F::S32),
        ("f32.aifc", PcmF32Be, F::F32),
        ("f64.aifc", PcmF64Be, F::F64),
    ];
    for (name, codec, format) in cases {
        for seekable in [true, false] {
            let src = FaultySource::new(Cursor::new(fixture(name)));
            let src: Box<dyn MediaSource> = if seekable {
                Box::new(src)
            } else {
                Box::new(src.not_seekable())
            };
            let mut d = aiff::open(src, &Limits::default()).unwrap();
            let info = d.streams()[0].clone();
            assert_eq!(info.codec, codec, "{name}");
            assert_eq!(info.audio.format(), format, "{name}");
            assert_eq!(info.audio.sample_rate(), signal::RATE, "{name}");
            assert_eq!(info.audio.layout(), &ChannelLayout::stereo(), "{name}");
            assert_eq!(info.frames, Some(signal::FRAMES as u64), "{name}");
            assert_eq!(info.bits_per_raw_sample, None, "{name}");
            let got = decode_all(&info, d.as_mut());
            assert!(got == expected(format, &info), "{name} seekable={seekable}");
        }
    }
}
