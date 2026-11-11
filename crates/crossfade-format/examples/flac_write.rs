//! Writes a synthetic signal as `<out>/<case>.flac` plus the source PCM as `<out>/<case>.raw`
//! (signed little-endian, interleaved) so independent decoders can be compared byte for byte.
//!
//! `cargo run --release -p crossfade-format --example flac_write -- <case> <out dir> [level]`
//! Cases: mono8, st16, st24, surround, silence, noise, empty, tags.

use crossfade_codec::{EncodeOptions, encoder_for};
use crossfade_core::{
    AudioFrame, AudioSpec, ChannelLayout, CodecId, Packet, SampleFormat, Samples, StreamInfo, Tags,
    TimeBase,
};
use crossfade_format::{FormatId, create_muxer};
use std::fs::File;
use std::time::Instant;

#[derive(Clone, Copy)]
enum Kind {
    Tone,
    Silence,
    Noise,
}

struct Case {
    rate: u32,
    format: SampleFormat,
    channels: u16,
    seconds: f64,
    kind: Kind,
    tags: bool,
}

fn case(name: &str) -> Option<Case> {
    let c = |rate, format, channels, seconds, kind, tags| Case {
        rate,
        format,
        channels,
        seconds,
        kind,
        tags,
    };
    Some(match name {
        "mono8" => c(8000, SampleFormat::U8, 1, 5.0, Kind::Tone, false),
        "st16" => c(44100, SampleFormat::S16, 2, 60.0, Kind::Tone, false),
        "st24" => c(96000, SampleFormat::S24, 2, 10.0, Kind::Tone, false),
        "surround" => c(48000, SampleFormat::S16, 6, 5.0, Kind::Tone, false),
        "silence" => c(44100, SampleFormat::S16, 2, 3.0, Kind::Silence, false),
        "noise" => c(44100, SampleFormat::S16, 2, 3.0, Kind::Noise, false),
        "empty" => c(44100, SampleFormat::S16, 2, 0.0, Kind::Tone, false),
        "tags" => c(44100, SampleFormat::S16, 2, 2.0, Kind::Tone, true),
        _ => return None,
    })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (Some(name), Some(dir)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: flac_write <case> <out dir> [level]");
        std::process::exit(2);
    };
    let level = args.get(3).map(|l| l.parse::<u8>().expect("level"));
    let c = case(name).expect("unknown case");
    let bits: u32 = match c.format {
        SampleFormat::U8 => 8,
        SampleFormat::S16 => 16,
        _ => 24,
    };
    let channels = usize::from(c.channels);
    let frames = (f64::from(c.rate) * c.seconds) as usize;
    let amp = f64::from((1u32 << (bits - 1)) - 1);
    let mut state = 1u32;
    let samples: Vec<i32> = (0..frames * channels)
        .map(|i| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = f64::from(state >> 8) / f64::from(1u32 << 24) * 2.0 - 1.0;
            let t = (i / channels) as f64;
            let ch = (i % channels) as f64;
            match c.kind {
                Kind::Silence => 0,
                Kind::Noise => (noise * amp) as i32,
                Kind::Tone => {
                    let tone = (t * (0.06 + 0.013 * ch)).sin() * 0.5 + (t * 0.0007).sin() * 0.2;
                    ((tone + noise * 0.01) * amp) as i32
                }
            }
        })
        .collect();

    let raw: Vec<u8> = samples
        .iter()
        .flat_map(|s| s.to_le_bytes()[..(bits / 8) as usize].to_vec())
        .collect();
    std::fs::write(format!("{dir}/{name}.raw"), raw).unwrap();

    let spec = AudioSpec::new(
        c.rate,
        c.format,
        ChannelLayout::unpositioned(c.channels).unwrap(),
    )
    .unwrap();
    let started = Instant::now();
    let mut enc = encoder_for(
        CodecId::Flac,
        &spec,
        &EncodeOptions {
            compression_level: level,
            ..EncodeOptions::default()
        },
    )
    .unwrap();
    let mut info = StreamInfo::new(
        0,
        CodecId::Flac,
        spec.clone(),
        TimeBase::new(1, i64::from(c.rate)).unwrap(),
    );
    info.extradata = enc.extradata().to_vec();
    let mut tags = Tags::new();
    if c.tags {
        tags.push("TITLE", "Crossfade test tone");
        tags.push("ARTIST", "A");
        tags.push("ARTIST", "B");
        tags.push("ALBUM", "Ünïcode ☃");
    }
    let file = File::create(format!("{dir}/{name}.flac")).unwrap();
    let mut mux = create_muxer(FormatId::Flac, Box::new(file), &info, &tags).unwrap();
    let mut p = Packet::default();
    for piece in samples.chunks(4410 * channels) {
        let mut f = AudioFrame::new(&spec);
        f.samples = match c.format {
            SampleFormat::U8 => Samples::U8(piece.iter().map(|&v| (v + 128) as u8).collect()),
            SampleFormat::S16 => Samples::S16(piece.iter().map(|&v| v as i16).collect()),
            _ => Samples::S24(piece.to_vec()),
        };
        enc.send_frame(&f).unwrap();
        while enc.receive_packet(&mut p).unwrap() {
            mux.write_packet(&p).unwrap();
        }
    }
    enc.drain();
    while enc.receive_packet(&mut p).unwrap() {
        mux.write_packet(&p).unwrap();
    }
    mux.set_extradata(enc.extradata()).unwrap();
    mux.finish().unwrap();
    let size = std::fs::metadata(format!("{dir}/{name}.flac"))
        .unwrap()
        .len();
    println!(
        "{name} level {level:?}: {size} bytes in {:.3} s",
        started.elapsed().as_secs_f64()
    );
}
