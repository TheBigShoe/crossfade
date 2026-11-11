//! trim and fade through the whole engine: WAV in, the chain, WAV or FLAC out, read back.
mod common;

use common::{data_chunk, spec, wav_bytes};
use crossfade_core::{ChannelLayout, Error, Limits, SampleFormat};
use crossfade_dsp::filters::fade::Curve;
use crossfade_graph::filter::FilterSpec;
use crossfade_graph::{AudioSettings, CancelToken, Dither, Job, JobInput, JobOutput, run};
use crossfade_testkit::memory::MemoryIo;

const RATE: u32 = 44100;

fn src(n: usize) -> Vec<i16> {
    (0..n)
        .map(|i| (((i * 2741) % 50_000) as i32 - 25_000) as i16)
        .collect()
}

fn input_wav(x: &[i16]) -> Vec<u8> {
    let pcm: Vec<u8> = x.iter().flat_map(|s| s.to_le_bytes()).collect();
    wav_bytes(&spec(RATE, SampleFormat::S16, ChannelLayout::mono()), &pcm)
}

fn f32_of(wav: &[u8]) -> Vec<f32> {
    let (q, _) = data_chunk(wav).as_chunks::<4>();
    q.iter().map(|&b| f32::from_le_bytes(b)).collect()
}

fn trim(start: Option<f64>, end: Option<f64>) -> FilterSpec {
    FilterSpec::Trim {
        start_seconds: start,
        end_seconds: end,
        duration_seconds: None,
    }
}

fn fade(i: Option<f64>, o: Option<f64>) -> FilterSpec {
    FilterSpec::Fade {
        in_seconds: i,
        out_seconds: o,
        curve: Curve::Linear,
    }
}

/// Float output, so the chain's result is stored unquantised.
fn job(input: &str, output: &str, filters: Vec<FilterSpec>, format: Option<SampleFormat>) -> Job {
    Job {
        version: Job::VERSION,
        inputs: vec![JobInput { path: input.into() }],
        outputs: vec![JobOutput {
            path: output.into(),
            audio: AudioSettings {
                filters,
                dither: Dither::None,
                sample_format: format,
                ..AudioSettings::default()
            },
        }],
    }
}

fn convert(io: &mut MemoryIo, job: &Job, threads: usize) -> Result<(), Error> {
    let limits = Limits {
        max_threads: threads,
        ..Limits::default()
    };
    run(job, io, &limits, &CancelToken::new(), &mut |_| {})
}

fn through(x: &[i16], filters: Vec<FilterSpec>) -> Vec<f32> {
    let mut io = MemoryIo::new();
    io.insert("in.wav", input_wav(x));
    let j = job("in.wav", "out.wav", filters, Some(SampleFormat::F32));
    convert(&mut io, &j, 1).unwrap();
    f32_of(&io.get("out.wav").unwrap())
}

fn unit(s: i16) -> f64 {
    f64::from(s) / 32768.0
}

#[test]
fn one_second_to_two_seconds_is_exactly_the_source_frames() {
    let x = src(3 * RATE as usize);
    let got = through(&x, vec![trim(Some(1.0), Some(2.0))]);
    let want: Vec<f32> = x[44100..88200].iter().map(|&s| unit(s) as f32).collect();
    assert_eq!(got.len(), 44100);
    assert_eq!(got, want);
}

#[test]
fn trim_then_fade_fades_the_trimmed_piece() {
    let x = src(3 * RATE as usize);
    let got = through(
        &x,
        vec![trim(Some(1.0), Some(2.0)), fade(Some(0.01), Some(0.01))],
    );
    let n = 441u64;
    assert_eq!(got.len(), 44100);
    let piece = &x[44100..88200];
    let g = |k: u64| k as f64 / n as f64;
    assert_eq!(got[0], 0.0);
    assert_eq!(got[44099], 0.0);
    for k in [1usize, 100, 440] {
        assert_eq!(got[k], (unit(piece[k]) * g(k as u64)) as f32, "in {k}");
        let j = 44099 - k;
        assert_eq!(got[j], (unit(piece[j]) * g(k as u64)) as f32, "out {k}");
    }
    assert_eq!(
        got[441],
        unit(piece[441]) as f32,
        "untouched after the fade-in"
    );
}

#[test]
fn fade_then_trim_can_cut_the_fade_away() {
    let x = src(3 * RATE as usize);
    // The fade-in lasts the first second; the trim keeps 0.5 s to 1.5 s, so the piece starts
    // half way up the fade and the fade-out at the very end never reaches it.
    let got = through(
        &x,
        vec![fade(Some(1.0), Some(0.5)), trim(Some(0.5), Some(1.5))],
    );
    assert_eq!(got.len(), 44100);
    let g = |j: usize| j as f64 / 44100.0;
    for k in [0usize, 1, 22049, 22050, 44099] {
        let j = 22050 + k;
        let gain = if j < 44100 { g(j) } else { 1.0 };
        assert_eq!(got[k], (unit(x[j]) * gain) as f32, "k {k}");
    }
    assert_ne!(got[44099], 0.0);
}

fn flac_without_total(x: &[i16]) -> Vec<u8> {
    let mut io = MemoryIo::new();
    io.insert("in.wav", input_wav(x));
    convert(&mut io, &job("in.wav", "a.flac", vec![], None), 1).unwrap();
    let mut flac = io.get("a.flac").unwrap();
    assert_eq!(&flac[..4], b"fLaC");
    // STREAMINFO body starts at 8; the 36-bit total is the low nibble of byte 21 and 22..26.
    assert_ne!(flac[22..26], [0; 4], "the writer recorded a total");
    flac[21] &= 0xF0;
    flac[22..26].fill(0);
    flac
}

#[test]
fn fade_out_works_on_a_flac_whose_streaminfo_has_no_total() {
    let x = src(20_000);
    let mut io = MemoryIo::new();
    io.insert("in.flac", flac_without_total(&x));
    let j = job(
        "in.flac",
        "out.wav",
        vec![fade(None, Some(0.1))],
        Some(SampleFormat::F32),
    );
    convert(&mut io, &j, 1).unwrap();
    let got = f32_of(&io.get("out.wav").unwrap());
    assert_eq!(got.len(), 20_000);
    let n = 4410usize;
    assert_eq!(got[19_999], 0.0);
    let k = 1000usize;
    let j = 19_999 - k;
    assert_eq!(got[j], (unit(x[j]) * (k as f64 / n as f64)) as f32);
    assert_eq!(got[20_000 - n - 1], unit(x[20_000 - n - 1]) as f32);
}

#[test]
fn threads_never_change_the_bytes() {
    let x = src(300_000);
    let mut outs = Vec::new();
    for threads in [1, 0, 3] {
        let mut io = MemoryIo::new();
        io.insert("in.wav", input_wav(&x));
        let j = job(
            "in.wav",
            "out.flac",
            vec![trim(Some(0.5), Some(5.0)), fade(Some(0.25), Some(0.5))],
            None,
        );
        convert(&mut io, &j, threads).unwrap();
        outs.push(io.get("out.flac").unwrap());
    }
    assert_eq!(outs[0], outs[1], "1 thread against auto");
    assert_eq!(outs[0], outs[2], "1 thread against 3");
}

#[test]
fn a_start_past_the_end_of_the_input_is_an_empty_output_not_an_error() {
    let got = through(&src(1000), vec![trim(Some(5.0), None)]);
    assert!(got.is_empty());
}

#[test]
fn a_fade_out_over_ten_minutes_is_refused_before_anything_is_written() {
    let mut io = MemoryIo::new();
    io.insert("in.wav", input_wav(&src(1000)));
    let j = job(
        "in.wav",
        "out.wav",
        vec![fade(None, Some(601.0))],
        Some(SampleFormat::F32),
    );
    let err = convert(&mut io, &j, 1).unwrap_err();
    assert!(matches!(err, Error::Unsupported { .. }), "{err}");
    assert_eq!(io.get("out.wav"), None);
}
