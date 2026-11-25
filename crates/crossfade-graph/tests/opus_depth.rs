//! The depth the Opus encoder is told (`OPUS_SET_LSB_DEPTH`, as `opusenc` sets it): the
//! source's, through the plan's own rate and channel conversion, and of the first input of a
//! join; 24 bits for a float source or after a filter that scales the samples. The signal makes
//! it visible: loud tones with hiss at 1e-4, coded superwideband at 64 kb/s when the encoder
//! is told 16 bits (the hiss above 12 kHz is under 16 bits' noise floor) and fullband at 24.
mod common;

use common::{spec, wav_bytes};
use crossfade_core::{ChannelLayout, Limits, SampleFormat};
use crossfade_format::open_demuxer;
use crossfade_graph::{AudioSettings, CancelToken, Job, JobInput, JobOutput, run};
use crossfade_opus::{Bandwidth, Packet as OpusPacket};
use crossfade_testkit::memory::MemoryIo;
use std::io::Cursor;

/// Two seconds of stereo tones at -6 dB with white noise at 1e-4, at `rate`, on the 16-bit grid.
fn tones_and_hiss(rate: u32) -> Vec<i16> {
    let mut seed = 1u32;
    let mut out = vec![];
    for i in 0..2 * rate as usize {
        let t = i as f64 / f64::from(rate);
        for ch in 0..2 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = f64::from(seed >> 8) / f64::from(1u32 << 24) * 2.0 - 1.0;
            let hz = 440.0 + 110.0 * f64::from(ch);
            let v = 0.5 * (2.0 * std::f64::consts::PI * hz * t).sin() + 1e-4 * noise;
            out.push((v * 32768.0).round() as i16);
        }
    }
    out
}

fn wav(rate: u32, samples: &[i16], format: SampleFormat) -> Vec<u8> {
    let bytes: Vec<u8> = match format {
        SampleFormat::S16 => samples.iter().flat_map(|v| v.to_le_bytes()).collect(),
        SampleFormat::F32 => samples
            .iter()
            .flat_map(|&v| (f32::from(v) / 32768.0).to_le_bytes())
            .collect(),
        other => panic!("{other:?}"),
    };
    wav_bytes(&spec(rate, format, ChannelLayout::stereo()), &bytes)
}

/// The bandwidths of `out.opus`'s packets after the first ten (the analysis settles), less the
/// last: the end padding is digital silence, which libopus codes fullband.
fn convert(io: &mut MemoryIo, inputs: &[&str], filters: &str) -> Vec<Bandwidth> {
    let job = Job {
        version: Job::VERSION,
        inputs: inputs
            .iter()
            .map(|p| JobInput { path: (*p).into() })
            .collect(),
        outputs: vec![JobOutput {
            path: "out.opus".into(),
            audio: AudioSettings {
                bitrate: Some(64_000),
                filters: filters
                    .split(',')
                    .filter(|f| !f.is_empty())
                    .map(|f| f.parse().unwrap())
                    .collect(),
                ..AudioSettings::default()
            },
        }],
    };
    run(
        &job,
        io,
        &Limits::default(),
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap();
    let bytes = io.get("out.opus").unwrap();
    let (_, mut d) = open_demuxer(Box::new(Cursor::new(bytes)), &Limits::default()).unwrap();
    let mut p = crossfade_core::Packet::default();
    let mut bandwidths = vec![];
    while d.read_packet(&mut p).unwrap() {
        bandwidths.push(OpusPacket::parse(&p.data).unwrap().toc().bandwidth());
    }
    bandwidths.pop();
    bandwidths.split_off(10)
}

fn all(got: &[Bandwidth], want: Bandwidth, what: &str) {
    assert!(got.iter().all(|&b| b == want), "{what}: {got:?}");
}

#[test]
fn the_encoder_is_told_the_sources_depth() {
    let mut io = MemoryIo::new();
    io.insert(
        "16.wav",
        wav(48_000, &tones_and_hiss(48_000), SampleFormat::S16),
    );
    io.insert(
        "f32.wav",
        wav(48_000, &tones_and_hiss(48_000), SampleFormat::F32),
    );
    all(
        &convert(&mut io, &["16.wav"], ""),
        Bandwidth::SuperWide,
        "16-bit",
    );
    all(
        &convert(&mut io, &["f32.wav"], ""),
        Bandwidth::Full,
        "float",
    );
}

#[test]
fn the_sources_depth_holds_through_the_resample_to_48_khz_as_in_opusenc() {
    let mut io = MemoryIo::new();
    io.insert(
        "16.wav",
        wav(44_100, &tones_and_hiss(44_100), SampleFormat::S16),
    );
    all(
        &convert(&mut io, &["16.wav"], ""),
        Bandwidth::SuperWide,
        "44.1 kHz",
    );
    all(
        &convert(&mut io, &["16.wav"], "resample 48000Hz"),
        Bandwidth::SuperWide,
        "resample filter",
    );
}

#[test]
fn a_filter_that_scales_the_samples_takes_the_encoder_to_24_bits() {
    let mut io = MemoryIo::new();
    io.insert(
        "16.wav",
        wav(48_000, &tones_and_hiss(48_000), SampleFormat::S16),
    );
    all(
        &convert(&mut io, &["16.wav"], "gain -1dB"),
        Bandwidth::Full,
        "gain",
    );
}

#[test]
fn a_join_takes_its_first_inputs_depth_as_the_header_takes_its_rate() {
    let mut io = MemoryIo::new();
    io.insert(
        "16.wav",
        wav(48_000, &tones_and_hiss(48_000), SampleFormat::S16),
    );
    io.insert(
        "f32.wav",
        wav(48_000, &tones_and_hiss(48_000), SampleFormat::F32),
    );
    all(
        &convert(&mut io, &["16.wav", "f32.wav"], ""),
        Bandwidth::SuperWide,
        "16-bit first",
    );
    all(
        &convert(&mut io, &["f32.wav", "16.wav"], ""),
        Bandwidth::Full,
        "float first",
    );
}
