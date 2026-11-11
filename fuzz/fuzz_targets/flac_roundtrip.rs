#![no_main]
//! The encode/decode property: whatever PCM comes in, the FLAC reader and decoder give back
//! exactly the samples the encoder was handed. Panics by design when that fails.

use crossfade_codec::{EncodeOptions, decoder_for, encoder_for};
use crossfade_core::{
    AudioFrame, AudioSpec, ChannelLayout, CodecId, Limits, Packet, SampleFormat, Samples,
    StreamInfo, Tags, TimeBase,
};
use crossfade_format::{FormatId, create_muxer};
use crossfade_fuzz::{Pipe, SharedSink};
use libfuzzer_sys::fuzz_target;

const RATES: [u32; 8] = [8000, 11025, 44100, 48000, 96000, 192_000, 384_000, 1_048_575];
/// Keeps one run fast; blocks are at most 4096 samples, so this still spans several.
const MAX_FRAMES: usize = 20_000;

fuzz_target!(|data: &[u8]| {
    let [fmt, channels, rate, level, body @ ..] = data else {
        return;
    };
    let format = [
        SampleFormat::U8,
        SampleFormat::S16,
        SampleFormat::S24,
        SampleFormat::S32,
    ][usize::from(*fmt) % 4];
    let channels = u16::from(*channels % 8) + 1;
    let rate = RATES[usize::from(*rate) % RATES.len()];
    let width = format.bytes_per_sample();
    let per_frame = width * usize::from(channels);
    let frames = (body.len() / per_frame).min(MAX_FRAMES);
    let body = &body[..frames * per_frame];

    let samples = match format {
        SampleFormat::U8 => Samples::U8(body.to_vec()),
        SampleFormat::S16 => Samples::S16(
            body.chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]))
                .collect(),
        ),
        SampleFormat::S24 => Samples::S24(
            body.chunks_exact(3)
                .map(|c| i32::from_le_bytes([c[0], c[1], c[2], 0]) << 8 >> 8)
                .collect(),
        ),
        _ => Samples::S32(
            body.chunks_exact(4)
                .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
        ),
    };

    let spec =
        AudioSpec::new(rate, format, ChannelLayout::unpositioned(channels).unwrap()).unwrap();
    let mut enc = encoder_for(
        CodecId::Flac,
        &spec,
        &EncodeOptions {
            compression_level: Some(*level % 9),
            ..EncodeOptions::default()
        },
    )
    .unwrap();
    let mut info = StreamInfo::new(
        0,
        CodecId::Flac,
        spec.clone(),
        TimeBase::new(1, i64::from(rate)).unwrap(),
    );
    info.extradata = enc.extradata().to_vec();
    let sink = SharedSink::default();
    let mut mux =
        create_muxer(FormatId::Flac, Box::new(sink.clone()), &info, &Tags::new()).unwrap();

    let mut packet = Packet::default();
    let mut frame = AudioFrame::new(&spec);
    frame.samples = samples.clone();
    enc.send_frame(&frame).unwrap();
    enc.drain();
    while enc.receive_packet(&mut packet).unwrap() {
        mux.write_packet(&packet).unwrap();
    }
    mux.set_extradata(enc.extradata()).unwrap();
    mux.finish().unwrap();

    let bytes = sink.bytes();
    let mut demux = crossfade_format::flac::open(Box::new(Pipe::new(&bytes)), &Limits::default())
        .expect("our own output opens");
    let stream = demux.streams()[0].clone();
    let mut dec = decoder_for(&stream).unwrap();
    let mut out = AudioFrame::new(&stream.audio);
    let mut back: Option<Samples> = None;
    let mut append = |f: &AudioFrame| match (&mut back, &f.samples) {
        (None, s) => back = Some(s.clone()),
        (Some(Samples::U8(a)), Samples::U8(b)) => a.extend(b),
        (Some(Samples::S16(a)), Samples::S16(b)) => a.extend(b),
        (Some(Samples::S24(a)), Samples::S24(b)) => a.extend(b),
        (Some(Samples::S32(a)), Samples::S32(b)) => a.extend(b),
        _ => panic!("sample format changed"),
    };
    while demux.read_packet(&mut packet).unwrap() {
        dec.send_packet(&packet).unwrap();
        while dec.receive_frame(&mut out).unwrap() {
            append(&out);
        }
    }
    dec.drain();
    while dec.receive_frame(&mut out).unwrap() {
        append(&out);
    }
    if frames == 0 {
        assert!(back.is_none_or(|b| b.is_empty()));
    } else {
        assert!(
            back.as_ref() == Some(&samples),
            "decoded samples differ from the input"
        );
    }
});
