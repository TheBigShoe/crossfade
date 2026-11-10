//! Independent helpers: they build and parse WAV bytes without the engine.
#![allow(dead_code, unreachable_pub)]

use crossfade_core::{AudioSpec, ChannelLayout, SampleFormat};
use crossfade_graph::{AudioSettings, Job, JobInput, JobOutput};
use crossfade_testkit::{pcm, signal};

/// Canonical 44-byte-header PCM WAV (format tag 1, or 3 for float).
pub fn wav_bytes(spec: &AudioSpec, pcm: &[u8]) -> Vec<u8> {
    let channels = spec.layout().channels();
    let bytes_per_frame = u32::try_from(spec.bytes_per_frame()).unwrap();
    let tag: u16 = if spec.format().is_float() { 3 } else { 1 };
    let data_len = u32::try_from(pcm.len()).unwrap();
    let mut v = Vec::with_capacity(44 + pcm.len());
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&tag.to_le_bytes());
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&spec.sample_rate().to_le_bytes());
    v.extend_from_slice(&(spec.sample_rate() * bytes_per_frame).to_le_bytes());
    v.extend_from_slice(&u16::try_from(bytes_per_frame).unwrap().to_le_bytes());
    v.extend_from_slice(&u16::try_from(spec.format().bits()).unwrap().to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    v.extend_from_slice(pcm);
    v
}

/// Walks the RIFF chunks and returns the `data` payload.
pub fn data_chunk(bytes: &[u8]) -> &[u8] {
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = pos + 8;
        if id == b"data" {
            return &bytes[body..body + len];
        }
        pos = body + len + (len & 1);
    }
    panic!("no data chunk");
}

pub fn spec(rate: u32, format: SampleFormat, layout: ChannelLayout) -> AudioSpec {
    AudioSpec::new(rate, format, layout).unwrap()
}

/// A sine as exact samples in [-1, 1) plus its WAV encoding in `format`.
pub fn sine_wav(spec: &AudioSpec, frames: usize) -> (Vec<f64>, Vec<u8>) {
    let samples = signal::render(
        &signal::Signal::Sine {
            hz: 440.0,
            amplitude: 0.5,
        },
        spec,
        frames,
    )
    .unwrap();
    let wav = wav_bytes(spec, &pcm::encode_le(&samples, spec.format()));
    (samples, wav)
}

pub fn job(input: &str, output: &str, format: Option<SampleFormat>) -> Job {
    Job {
        version: Job::VERSION,
        inputs: vec![JobInput { path: input.into() }],
        outputs: vec![JobOutput {
            path: output.into(),
            audio: AudioSettings {
                sample_format: format,
                ..AudioSettings::default()
            },
        }],
    }
}
