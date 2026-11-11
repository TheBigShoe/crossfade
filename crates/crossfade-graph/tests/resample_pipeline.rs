//! Sample-rate conversion through the whole engine: real jobs, real containers, read back.
mod common;

use common::{data_chunk, spec, wav_bytes};
use crossfade_core::{ChannelLayout, Limits, SampleFormat};
use crossfade_graph::filter::FilterSpec;
use crossfade_graph::{AudioSettings, CancelToken, Job, JobInput, JobOutput, probe, run};
use crossfade_resample::output_len;
use crossfade_testkit::memory::MemoryIo;
use crossfade_testkit::pcm;

const FS: u32 = 44_100;

fn job(input: &str, output: &str, audio: AudioSettings) -> Job {
    Job {
        version: Job::VERSION,
        inputs: vec![JobInput { path: input.into() }],
        outputs: vec![JobOutput {
            path: output.into(),
            audio,
        }],
    }
}

fn convert(io: &mut MemoryIo, input: &str, output: &str, audio: AudioSettings, threads: usize) {
    let limits = Limits {
        max_threads: threads,
        ..Limits::default()
    };
    run(
        &job(input, output, audio),
        io,
        &limits,
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap_or_else(|e| panic!("{input} -> {output}: {e}"));
}

fn resample_to(rate: u32, quality: &str, format: SampleFormat) -> AudioSettings {
    AudioSettings {
        sample_format: Some(format),
        filters: vec![
            format!("resample {rate} quality={quality}")
                .parse::<FilterSpec>()
                .unwrap(),
        ],
        ..AudioSettings::default()
    }
}

/// A 1 kHz sine at -1 dBFS, `frames` long, as f64 samples and an f64 WAV.
fn sine(frames: usize) -> (Vec<f64>, Vec<u8>) {
    let a = libm::pow(10.0, -1.0 / 20.0);
    let x: Vec<f64> = (0..frames)
        .map(|n| a * libm::sin(2.0 * std::f64::consts::PI * 1000.0 * n as f64 / f64::from(FS)))
        .collect();
    let s = spec(FS, SampleFormat::F64, ChannelLayout::mono());
    let wav = wav_bytes(&s, &pcm::encode_le(&x, SampleFormat::F64));
    (x, wav)
}

fn f64s(wav: &[u8]) -> Vec<f64> {
    pcm::decode_le(data_chunk(wav), SampleFormat::F64).unwrap()
}

/// Signal to error, in dB, away from the ends (a sine that starts and stops abruptly rings
/// for the length of the kernel there).
fn snr_db(x: &[f64], y: &[f64]) -> f64 {
    let edge = FS as usize / 10;
    assert_eq!(x.len(), y.len());
    let (x, y) = (&x[edge..x.len() - edge], &y[edge..y.len() - edge]);
    let (mut s, mut e) = (0.0, 0.0);
    for (a, b) in x.iter().zip(y) {
        s += a * a;
        e += (a - b) * (a - b);
    }
    10.0 * libm::log10(s / e)
}

/// 44.1 kHz -> 48 kHz in `mid`, then back to 44.1 kHz f64, as two separate jobs.
fn round_trip(quality: &str, mid: SampleFormat) -> f64 {
    let (x, wav) = sine(2 * FS as usize);
    let mut io = MemoryIo::new();
    io.insert("src.wav", wav);
    convert(
        &mut io,
        "src.wav",
        "mid.wav",
        resample_to(48_000, quality, mid),
        1,
    );
    let back = AudioSettings {
        sample_format: Some(SampleFormat::F64),
        ..resample_to(FS, quality, SampleFormat::F64)
    };
    convert(&mut io, "mid.wav", "back.wav", back, 1);
    let y = f64s(&io.get("back.wav").unwrap());
    assert_eq!(y.len(), x.len(), "88200 -> 96000 -> 88200 frames");
    snr_db(&x, &y)
}

/// What one s24 quantisation (with the engine's dither) of the same sine allows.
fn s24_limit() -> f64 {
    let (x, wav) = sine(2 * FS as usize);
    let mut io = MemoryIo::new();
    io.insert("src.wav", wav);
    let unity = AudioSettings {
        sample_format: Some(SampleFormat::S24),
        filters: vec!["gain 0dB".parse().unwrap()],
        ..AudioSettings::default()
    };
    convert(&mut io, "src.wav", "q.wav", unity, 1);
    let back = AudioSettings {
        sample_format: Some(SampleFormat::F64),
        ..AudioSettings::default()
    };
    convert(&mut io, "q.wav", "qf.wav", back, 1);
    snr_db(&x, &f64s(&io.get("qf.wav").unwrap()))
}

#[test]
fn round_trip_44k1_48k_44k1_snr_exit_criterion() {
    let limit = s24_limit();
    eprintln!("s24 quantisation alone: {limit:.1} dB");
    for quality in ["fast", "good", "best"] {
        for (mid, floor) in [
            (SampleFormat::F64, 110.0),
            (SampleFormat::F32, 110.0),
            (SampleFormat::S24, limit - 1.0),
        ] {
            let snr = round_trip(quality, mid);
            eprintln!("{quality} via {}: {snr:.1} dB", mid.name());
            assert!(
                snr >= floor,
                "{quality} via {mid:?}: {snr:.1} dB < {floor:.1}"
            );
        }
    }
}

#[test]
fn the_output_rate_setting_converts_at_the_default_quality() {
    let (_, wav) = sine(FS as usize);
    let mut io = MemoryIo::new();
    io.insert("src.wav", wav);
    let by_setting = AudioSettings {
        sample_rate: Some(48_000),
        ..AudioSettings::default()
    };
    convert(&mut io, "src.wav", "a.wav", by_setting, 1);
    convert(
        &mut io,
        "src.wav",
        "b.wav",
        resample_to(48_000, "good", SampleFormat::F64),
        1,
    );
    assert_eq!(io.get("a.wav"), io.get("b.wav"));
}

fn s16_source(frames: usize) -> Vec<u8> {
    let x: Vec<f64> = (0..frames * 2)
        .map(|i| 0.6 * libm::sin(i as f64 * 0.0123) * libm::cos(i as f64 * 0.000_77))
        .collect();
    let s = spec(FS, SampleFormat::S16, ChannelLayout::stereo());
    wav_bytes(&s, &pcm::encode_le(&x, SampleFormat::S16))
}

#[test]
fn every_container_holds_the_new_rate_and_length() {
    let mut io = MemoryIo::new();
    io.insert("src.wav", s16_source(44_100 + 17));
    let want = output_len(44_100 + 17, FS, 48_000);
    for out in ["o.wav", "o.flac", "o.aiff", "o.oga"] {
        let at_48k = AudioSettings {
            sample_rate: Some(48_000),
            ..AudioSettings::default()
        };
        convert(&mut io, "src.wav", out, at_48k, 0);
        let report = probe(out, &mut io, &Limits::default()).unwrap();
        let s = &report.streams[0];
        assert_eq!(s.audio.sample_rate(), 48_000, "{out}");
        assert_eq!(s.frames, Some(want), "{out}");
        // Decoding it all checks FLAC's STREAMINFO total and MD5 against the audio.
        let back = format!("{out}.wav");
        convert(&mut io, out, &back, AudioSettings::default(), 0);
        let n = data_chunk(&io.get(&back).unwrap()).len() as u64 / 4;
        assert_eq!(n, want, "{out}");
    }
}

#[test]
fn thread_count_and_framing_never_change_the_bytes() {
    let mut io = MemoryIo::new();
    io.insert("src.wav", s16_source(200_000));
    convert(&mut io, "src.wav", "src.flac", AudioSettings::default(), 0);
    let to = |format| AudioSettings {
        sample_rate: Some(48_000),
        sample_format: Some(format),
        ..AudioSettings::default()
    };
    let mut flacs = Vec::new();
    for threads in [1, 0, 3] {
        convert(
            &mut io,
            "src.flac",
            "o.flac",
            to(SampleFormat::S16),
            threads,
        );
        flacs.push(io.get("o.flac").unwrap());
    }
    assert_eq!(flacs[0], flacs[1], "1 thread against auto");
    assert_eq!(flacs[0], flacs[2], "1 thread against 3");
    // A WAV and a FLAC of the same audio decode in different frame sizes.
    convert(&mut io, "src.wav", "a.wav", to(SampleFormat::F64), 1);
    convert(&mut io, "src.flac", "b.wav", to(SampleFormat::F64), 1);
    assert_eq!(io.get("a.wav"), io.get("b.wav"));
}
