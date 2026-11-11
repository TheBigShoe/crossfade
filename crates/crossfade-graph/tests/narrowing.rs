//! Narrowing without filters: an integer output narrower than the input's true depth loses
//! real bits, so it is dithered (TPDF unless the job says `none`); `none` keeps the old bytes.
//! A narrowing that loses nothing (16 real bits in a 24-bit container to s16), widening, and
//! anything to float stay exactly as they were.
mod common;

use common::{data_chunk, spec, wav_bytes};
use crossfade_core::{ChannelLayout, Limits, SampleFormat};
use crossfade_graph::{
    AudioSettings, CancelToken, Dither, Job, JobInput, JobOutput, describe, probe, run,
};
use crossfade_testkit::memory::MemoryIo;

fn convert(io: &mut MemoryIo, input: &str, output: &str, audio: AudioSettings) {
    let job = Job {
        version: Job::VERSION,
        inputs: vec![JobInput { path: input.into() }],
        outputs: vec![JobOutput {
            path: output.into(),
            audio,
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
}

fn to(format: SampleFormat, dither: Dither) -> AudioSettings {
    AudioSettings {
        sample_format: Some(format),
        dither,
        ..AudioSettings::default()
    }
}

fn s24_samples(n: usize) -> Vec<i32> {
    (0..n)
        .map(|i| ((i as i64 * 2_654_435_761 % 16_777_216) - 8_388_608) as i32)
        .collect()
}

fn s24_wav(samples: &[i32]) -> Vec<u8> {
    let pcm: Vec<u8> = samples
        .iter()
        .flat_map(|s| s.to_le_bytes()[..3].to_vec())
        .collect();
    wav_bytes(
        &spec(48_000, SampleFormat::S24, ChannelLayout::stereo()),
        &pcm,
    )
}

fn s16_of(wav: &[u8]) -> Vec<i16> {
    data_chunk(wav)
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&b| i16::from_le_bytes(b))
        .collect()
}

/// WAVE_FORMAT_EXTENSIBLE, 24-bit container, `valid` real bits, stereo.
fn extensible_s24(valid: u16, samples: &[i32]) -> Vec<u8> {
    let pcm: Vec<u8> = samples
        .iter()
        .flat_map(|s| s.to_le_bytes()[..3].to_vec())
        .collect();
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&0xFFFEu16.to_le_bytes());
    fmt.extend_from_slice(&2u16.to_le_bytes());
    fmt.extend_from_slice(&48_000u32.to_le_bytes());
    fmt.extend_from_slice(&(48_000u32 * 6).to_le_bytes());
    fmt.extend_from_slice(&6u16.to_le_bytes());
    fmt.extend_from_slice(&24u16.to_le_bytes());
    fmt.extend_from_slice(&22u16.to_le_bytes());
    fmt.extend_from_slice(&valid.to_le_bytes());
    fmt.extend_from_slice(&3u32.to_le_bytes());
    fmt.extend_from_slice(&[
        1, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71,
    ]);
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(4 + 8 + fmt.len() as u32 + 8 + pcm.len() as u32).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
    v.extend_from_slice(&fmt);
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    v.extend_from_slice(&pcm);
    v
}

#[test]
fn twenty_four_real_bits_to_s16_are_dithered_and_none_keeps_the_old_truncation() {
    let x = s24_samples(20_000);
    let mut io = MemoryIo::new();
    io.insert("in.wav", s24_wav(&x));
    convert(
        &mut io,
        "in.wav",
        "none.wav",
        to(SampleFormat::S16, Dither::None),
    );
    let truncated: Vec<i16> = x.iter().map(|&s| (s >> 8) as i16).collect();
    assert_eq!(s16_of(&io.get("none.wav").unwrap()), truncated);
    convert(
        &mut io,
        "in.wav",
        "tpdf.wav",
        to(SampleFormat::S16, Dither::Tpdf),
    );
    let dithered = s16_of(&io.get("tpdf.wav").unwrap());
    assert_ne!(dithered, truncated);
    // Every sample is within one step of the exact value, and the error has no offset.
    let mut sum = 0.0;
    for (&d, &s) in dithered.iter().zip(&x) {
        let exact = f64::from(s) / 256.0;
        let e = f64::from(d) - exact;
        assert!(e.abs() < 1.5, "{d} for {exact}");
        sum += e;
    }
    let mean = sum / x.len() as f64;
    assert!(mean.abs() < 0.02, "mean error {mean} LSB");
}

#[test]
fn float_to_integer_is_dithered_and_none_keeps_the_old_rounding() {
    let x: Vec<f64> = (0..10_000)
        .map(|i| 0.7 * libm::sin(f64::from(i) * 0.01))
        .collect();
    let s = spec(48_000, SampleFormat::F32, ChannelLayout::mono());
    let pcm = crossfade_testkit::pcm::encode_le(&x, SampleFormat::F32);
    let mut io = MemoryIo::new();
    io.insert("in.wav", wav_bytes(&s, &pcm));
    convert(
        &mut io,
        "in.wav",
        "none.wav",
        to(SampleFormat::S16, Dither::None),
    );
    let rounded: Vec<i16> = crossfade_testkit::pcm::decode_le(&pcm, SampleFormat::F32)
        .unwrap()
        .iter()
        .map(|&v| (v * 32768.0).round().clamp(-32768.0, 32767.0) as i16)
        .collect();
    assert_eq!(s16_of(&io.get("none.wav").unwrap()), rounded);
    convert(
        &mut io,
        "in.wav",
        "tpdf.wav",
        AudioSettings {
            sample_format: Some(SampleFormat::S16),
            ..AudioSettings::default()
        },
    );
    assert_ne!(s16_of(&io.get("tpdf.wav").unwrap()), rounded);
}

#[test]
fn sixteen_real_bits_in_a_24_bit_container_narrow_to_s16_exactly() {
    let x: Vec<i32> = s24_samples(5000).iter().map(|s| s & !0xFF).collect();
    let mut io = MemoryIo::new();
    io.insert("in.wav", extensible_s24(16, &x));
    let stream = &probe("in.wav", &mut io, &Limits::default())
        .unwrap()
        .streams[0];
    assert_eq!(stream.bits_per_raw_sample, Some(16));
    convert(
        &mut io,
        "in.wav",
        "out.wav",
        to(SampleFormat::S16, Dither::Tpdf),
    );
    let want: Vec<i16> = x.iter().map(|&s| (s >> 8) as i16).collect();
    assert_eq!(s16_of(&io.get("out.wav").unwrap()), want);
}

#[test]
fn widening_same_depth_and_float_outputs_are_untouched() {
    let x = s24_samples(5000);
    let mut io = MemoryIo::new();
    io.insert("in.wav", s24_wav(&x));
    let plain = |io: &mut MemoryIo, name: &str, format| {
        convert(io, "in.wav", name, to(format, Dither::None));
        let none = io.get(name).unwrap();
        convert(io, "in.wav", name, to(format, Dither::Tpdf));
        assert_eq!(io.get(name).unwrap(), none, "{format:?}");
    };
    for format in [
        SampleFormat::S24,
        SampleFormat::S32,
        SampleFormat::F32,
        SampleFormat::F64,
    ] {
        plain(&mut io, "o.wav", format);
    }
}

#[test]
fn describe_reports_the_full_width_of_a_dithered_narrowing() {
    let mut io = MemoryIo::new();
    io.insert("in.wav", s24_wav(&s24_samples(100)));
    let stream = &probe("in.wav", &mut io, &Limits::default())
        .unwrap()
        .streams[0];
    let plan = describe(stream, "o.flac", &to(SampleFormat::S16, Dither::Tpdf)).unwrap();
    assert_eq!(plan.output.bits_per_raw_sample, None);
    assert!(
        crossfade_graph::processing(stream, &to(SampleFormat::S16, Dither::Tpdf))
            .unwrap()
            .is_some()
    );
    assert!(
        crossfade_graph::processing(stream, &to(SampleFormat::S16, Dither::None))
            .unwrap()
            .is_none()
    );
    assert!(
        crossfade_graph::processing(stream, &to(SampleFormat::S32, Dither::Tpdf))
            .unwrap()
            .is_none()
    );
}
