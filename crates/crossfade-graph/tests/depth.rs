//! A file's true bit depth survives conversion between FLAC, WAV and AIFF. Sources are written
//! here with the engine's own encoder and muxers; no external tool is involved.
mod common;

use common::{job, sine_wav, spec};
use crossfade_codec::{EncodeOptions, decoder_for, encoder_for};
use crossfade_core::{
    AudioFrame, ChannelLayout, CodecId, JobIo, Limits, Packet, SampleFormat, Samples, StreamInfo,
    Tags, TimeBase,
};
use crossfade_format::{FormatId, create_muxer, open_demuxer};
use crossfade_graph::{AudioSettings, CancelToken, run};
use crossfade_testkit::memory::MemoryIo;
use std::io::Cursor;

const FRAMES: usize = 5000;

fn container(depth: u8) -> SampleFormat {
    match depth {
        0..=8 => SampleFormat::U8,
        9..=16 => SampleFormat::S16,
        17..=24 => SampleFormat::S24,
        _ => SampleFormat::S32,
    }
}

/// Noise of exactly `depth` bits, right-justified, so every depth uses its full range.
fn right_justified(depth: u8, n: usize) -> Vec<i32> {
    let mut state = 0x9E37_79B9_7F4A_7C15u64 ^ u64::from(depth);
    (0..n)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((state as i64) >> (64 - u32::from(depth))) as i32
        })
        .collect()
}

fn frame_of(spec: &crossfade_core::AudioSpec, left: &[i32]) -> AudioFrame {
    let mut f = AudioFrame::new(spec);
    f.samples = match spec.format() {
        SampleFormat::U8 => Samples::U8(left.iter().map(|&v| (v + 128) as u8).collect()),
        SampleFormat::S16 => Samples::S16(left.iter().map(|&v| v as i16).collect()),
        SampleFormat::S24 => Samples::S24(left.to_vec()),
        SampleFormat::S32 => Samples::S32(left.to_vec()),
        other => panic!("{other:?}"),
    };
    f
}

/// A stereo FLAC of `depth` bits made by the encoder and the FLAC muxer.
fn source_flac(depth: u8) -> Vec<u8> {
    let format = container(depth);
    let audio = spec(48000, format, ChannelLayout::stereo());
    let width = format.bits() as u8;
    let left: Vec<i32> = right_justified(depth, FRAMES * 2)
        .iter()
        .map(|&v| v << (width - depth))
        .collect();
    let options = EncodeOptions {
        bits_per_sample: Some(depth),
        ..EncodeOptions::default()
    };
    let mut enc = encoder_for(CodecId::Flac, &audio, &options).unwrap();
    let stream = StreamInfo {
        frames: Some(FRAMES as u64),
        bits_per_raw_sample: (depth != width).then_some(depth),
        ..StreamInfo::new(
            0,
            CodecId::Flac,
            audio.clone(),
            TimeBase::new(1, 48000).unwrap(),
        )
    };
    let mut io = MemoryIo::new();
    let mut mux = create_muxer(
        FormatId::Flac,
        io.create_sink("s.flac").unwrap(),
        &stream,
        &Tags::new(),
    )
    .unwrap();
    enc.send_frame(&frame_of(&audio, &left)).unwrap();
    enc.drain();
    let mut p = Packet::default();
    while enc.receive_packet(&mut p).unwrap() {
        mux.write_packet(&p).unwrap();
    }
    mux.set_extradata(enc.extradata()).unwrap();
    mux.finish().unwrap();
    io.commit("s.flac").unwrap();
    io.get("s.flac").unwrap()
}

fn convert(io: &mut MemoryIo, from: &str, to: &str, audio: AudioSettings) {
    let mut j = job(from, to, None);
    j.outputs[0].audio = audio;
    run(&j, io, &Limits::default(), &CancelToken::new(), &mut |_| {})
        .unwrap_or_else(|e| panic!("{from} -> {to}: {e}"));
}

/// What the file says its depth is, and (FLAC) its STREAMINFO MD5.
struct Read {
    raw: Option<u8>,
    format: SampleFormat,
    streaminfo: Option<(u8, [u8; 16])>,
    samples: Vec<i32>,
}

fn read(bytes: Vec<u8>) -> Read {
    let (_, mut d) = open_demuxer(Box::new(Cursor::new(bytes)), &Limits::default()).unwrap();
    let stream = d.streams()[0].clone();
    let streaminfo = (stream.codec == CodecId::Flac).then(|| {
        let e = &stream.extradata;
        let bits = (((e[12] & 1) << 4) | (e[13] >> 4)) + 1;
        (bits, e[18..34].try_into().unwrap())
    });
    let mut dec = decoder_for(&stream).unwrap();
    let mut frame = AudioFrame::new(&stream.audio);
    let mut p = Packet::default();
    let mut samples = Vec::new();
    let mut take = |dec: &mut Box<dyn crossfade_core::Decoder>, frame: &mut AudioFrame| {
        while dec.receive_frame(frame).unwrap() {
            match &frame.samples {
                Samples::U8(v) => samples.extend(v.iter().map(|&x| i32::from(x) - 128)),
                Samples::S16(v) => samples.extend(v.iter().map(|&x| i32::from(x))),
                Samples::S24(v) | Samples::S32(v) => samples.extend(v),
                other => panic!("{other:?}"),
            }
        }
    };
    while d.read_packet(&mut p).unwrap() {
        dec.send_packet(&p).unwrap();
        take(&mut dec, &mut frame);
    }
    dec.drain();
    take(&mut dec, &mut frame);
    Read {
        raw: stream.bits_per_raw_sample,
        format: stream.audio.format(),
        streaminfo,
        samples,
    }
}

const DEPTHS: [u8; 12] = [4, 6, 8, 9, 12, 16, 17, 20, 24, 25, 28, 32];

#[test]
fn flac_to_flac_keeps_the_depth_the_samples_and_the_md5() {
    for depth in DEPTHS {
        let src = source_flac(depth);
        let before = read(src.clone());
        assert_eq!(before.streaminfo.unwrap().0, depth, "source {depth}");
        let mut io = MemoryIo::new();
        io.insert("in.flac", src);
        convert(&mut io, "in.flac", "out.flac", AudioSettings::default());
        let after = read(io.get("out.flac").unwrap());
        assert_eq!(after.streaminfo, before.streaminfo, "{depth}");
        assert_eq!(after.raw, before.raw, "{depth}");
        assert_eq!(after.format, before.format, "{depth}");
        assert_eq!(after.samples, before.samples, "{depth}");
    }
}

#[test]
fn asking_for_a_wider_format_writes_that_width() {
    let audio = spec(8000, SampleFormat::S16, ChannelLayout::stereo());
    let mut io = MemoryIo::new();
    io.insert("in.wav", sine_wav(&audio, 3000).1);
    let wider = AudioSettings {
        sample_format: Some(SampleFormat::S24),
        ..AudioSettings::default()
    };
    convert(&mut io, "in.wav", "out.flac", wider);
    assert_eq!(read(io.get("out.flac").unwrap()).streaminfo.unwrap().0, 24);
}

#[test]
fn a_depth_survives_a_round_trip_through_wav_and_aiff() {
    for depth in DEPTHS {
        let src = source_flac(depth);
        let before = read(src.clone());
        for ext in ["wav", "aiff"] {
            let mut io = MemoryIo::new();
            io.insert("in.flac", src.clone());
            let mid = format!("mid.{ext}");
            convert(&mut io, "in.flac", &mid, AudioSettings::default());
            let middle = read(io.get(&mid).unwrap());
            assert_eq!(middle.raw, before.raw, "{depth} via {ext}");
            assert_eq!(middle.samples, before.samples, "{depth} via {ext}");
            convert(&mut io, &mid, "out.flac", AudioSettings::default());
            let after = read(io.get("out.flac").unwrap());
            assert_eq!(after.streaminfo, before.streaminfo, "{depth} via {ext}");
            assert_eq!(after.samples, before.samples, "{depth} via {ext}");
        }
    }
}

#[test]
fn a_changed_sample_format_writes_the_full_width() {
    let mut io = MemoryIo::new();
    io.insert("in.flac", source_flac(20));
    for (format, ext, width) in [
        (SampleFormat::S16, "flac", 16),
        (SampleFormat::S32, "flac", 32),
    ] {
        let to = format!("o{width}.{ext}");
        let settings = AudioSettings {
            sample_format: Some(format),
            ..AudioSettings::default()
        };
        convert(&mut io, "in.flac", &to, settings);
        let out = read(io.get(&to).unwrap());
        assert_eq!(out.streaminfo.unwrap().0, width, "{format:?}");
        assert_eq!(out.raw, None, "{format:?}");
    }
}

#[test]
fn stream_copy_keeps_the_stream_as_it_is() {
    let src = source_flac(20);
    let mut io = MemoryIo::new();
    io.insert("in.flac", src.clone());
    let copy = AudioSettings {
        codec: Some("copy".into()),
        ..AudioSettings::default()
    };
    convert(&mut io, "in.flac", "out.flac", copy);
    let (a, b) = (read(src), read(io.get("out.flac").unwrap()));
    assert_eq!(a.streaminfo, b.streaminfo);
    assert_eq!(b.raw, Some(20));
}
