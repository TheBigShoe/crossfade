//! Two-pass loudness normalisation through the engine: pass one measures the signal as it
//! reaches the `normalize` stage, pass two converts with the gain that gives. The achieved
//! loudness is checked by measuring the OUTPUT file with the same meter.
mod common;

use common::{spec, wav_bytes};
use crossfade_core::{ChannelLayout, Error, JobIo, Limits, MediaSink, MediaSource, SampleFormat};
use crossfade_graph::filter::FilterSpec;
use crossfade_graph::{
    AudioSettings, CancelToken, Event, Job, JobInput, JobOutput, Pipeline, describe,
    measure_loudness, probe, run, validate,
};
use crossfade_loudness::to_db;
use crossfade_testkit::memory::MemoryIo;
use crossfade_testkit::pcm;
use std::collections::HashMap;

const FS: u32 = 44_100;

fn job(inputs: &[&str], output: &str, audio: AudioSettings) -> Job {
    Job {
        version: Job::VERSION,
        inputs: inputs
            .iter()
            .map(|p| JobInput { path: (*p).into() })
            .collect(),
        outputs: vec![JobOutput {
            path: output.into(),
            audio,
        }],
    }
}

fn settings(filters: &[&str]) -> AudioSettings {
    AudioSettings {
        sample_format: Some(SampleFormat::F64),
        filters: filters
            .iter()
            .map(|f| f.parse::<FilterSpec>().unwrap())
            .collect(),
        ..AudioSettings::default()
    }
}

fn convert(
    io: &mut dyn JobIo,
    inputs: &[&str],
    output: &str,
    audio: AudioSettings,
    threads: usize,
) -> Result<Vec<Event>, Error> {
    let mut events = Vec::new();
    let limits = Limits {
        max_threads: threads,
        ..Limits::default()
    };
    run(
        &job(inputs, output, audio),
        io,
        &limits,
        &CancelToken::new(),
        &mut |e| {
            events.push(e.clone());
        },
    )?;
    Ok(events)
}

/// Stereo programme: a warbling tone at `amplitude`, `seconds` long, at `rate`.
fn programme(rate: u32, seconds: f64, amplitude: f64) -> Vec<f64> {
    let n = (seconds * f64::from(rate)) as usize;
    (0..n * 2)
        .map(|i| {
            let t = (i / 2) as f64 / f64::from(rate);
            let f = 700.0 + 300.0 * libm::sin(t * 1.3);
            amplitude * libm::sin(2.0 * std::f64::consts::PI * f * t + (i % 2) as f64)
        })
        .collect()
}

fn f64_wav(rate: u32, x: &[f64]) -> Vec<u8> {
    wav_bytes(
        &spec(rate, SampleFormat::F64, ChannelLayout::stereo()),
        &pcm::encode_le(x, SampleFormat::F64),
    )
}

fn loudness(io: &mut MemoryIo, path: &str) -> crossfade_graph::Measurements {
    measure_loudness(path, io, &Limits::default(), &CancelToken::new()).unwrap()
}

fn normalized(events: &[Event]) -> &crossfade_graph::Normalization {
    events
        .iter()
        .find_map(|e| match e {
            Event::Normalized(n) => Some(n),
            _ => None,
        })
        .expect("a Normalized event")
}

#[test]
fn the_output_reaches_the_target_within_a_tenth_of_a_lu() {
    let mut io = MemoryIo::new();
    io.insert("quiet.wav", f64_wav(FS, &programme(FS, 8.0, 0.02)));
    let events = convert(
        &mut io,
        &["quiet.wav"],
        "out.wav",
        settings(&["normalize -16LUFS"]),
        1,
    )
    .unwrap();
    let n = normalized(&events);
    assert!(!n.limited_by_ceiling);
    let measured = loudness(&mut io, "out.wav").integrated.unwrap();
    assert!((measured + 16.0).abs() < 0.1, "{measured} LUFS");
    let reached = n.reached_lufs().unwrap();
    assert!(
        (reached - measured).abs() < 0.01,
        "{reached} predicted, {measured} measured"
    );
}

#[test]
fn a_ceiling_limited_gain_is_reported_and_the_peak_stays_under_it() {
    // A quiet tone with full-scale clicks: the clicks, not the loudness, set the gain.
    let mut x = programme(FS, 8.0, 0.01);
    for k in (0..x.len()).step_by(40_000) {
        x[k] = 0.5;
    }
    let mut io = MemoryIo::new();
    io.insert("peaky.wav", f64_wav(FS, &x));
    let events = convert(
        &mut io,
        &["peaky.wav"],
        "out.wav",
        settings(&["normalize -14LUFS peak=-1dBTP"]),
        1,
    )
    .unwrap();
    let n = normalized(&events);
    assert!(n.limited_by_ceiling);
    let m = loudness(&mut io, "out.wav");
    // The same meter on a signal that is the input times one gain: the true peak scales
    // exactly, so only f64 rounding separates it from the ceiling.
    let peak = to_db(m.true_peak.unwrap());
    assert!(peak <= -1.0 + 1e-6, "{peak} dBTP");
    assert!(
        (peak + 1.0).abs() < 1e-6,
        "the ceiling sets the gain exactly: {peak}"
    );
    let reached = n.reached_lufs().unwrap();
    assert!((reached - m.integrated.unwrap()).abs() < 0.01);
    assert!(reached < -14.0 - 1.0, "short of the target: {reached}");
}

#[test]
fn silence_gets_unity_and_says_so() {
    let mut io = MemoryIo::new();
    io.insert("silence.wav", f64_wav(FS, &vec![0.0; 2 * 44_100]));
    let events = convert(
        &mut io,
        &["silence.wav"],
        "out.wav",
        settings(&["normalize -16LUFS"]),
        1,
    )
    .unwrap();
    let n = normalized(&events);
    assert_eq!(n.measured_lufs, None);
    assert_eq!(n.gain_db, 0.0);
    assert_eq!(
        common::data_chunk(&io.get("out.wav").unwrap()),
        common::data_chunk(&io.get("silence.wav").unwrap())
    );
}

#[test]
fn progress_runs_once_through_both_passes() {
    let mut io = MemoryIo::new();
    io.insert("in.wav", f64_wav(FS, &programme(FS, 3.0, 0.05)));
    let events = convert(
        &mut io,
        &["in.wav"],
        "out.wav",
        settings(&["normalize -20LUFS"]),
        1,
    )
    .unwrap();
    assert_eq!(events[0], Event::Measuring);
    let normalized_at = events
        .iter()
        .position(|e| matches!(e, Event::Normalized(_)))
        .unwrap();
    let started_at = events
        .iter()
        .position(|e| matches!(e, Event::Started { .. }))
        .unwrap();
    assert!(normalized_at < started_at);
    let progress: Vec<(u64, Option<u64>)> = events
        .iter()
        .filter_map(|e| match e {
            Event::Progress {
                frames_done,
                frames_total,
            } => Some((*frames_done, *frames_total)),
            _ => None,
        })
        .collect();
    let total = 2 * 3 * u64::from(FS);
    assert!(progress.iter().all(|p| p.1 == Some(total)));
    assert!(progress.windows(2).all(|w| w[0].0 <= w[1].0));
    assert!(
        progress.iter().any(|p| p.0 == total / 2),
        "pass one ends at half"
    );
    assert_eq!(progress.last().unwrap().0, total);
    assert_eq!(events.last(), Some(&Event::Finished { frames: total }));
}

#[test]
fn a_join_is_measured_as_joined() {
    let mut io = MemoryIo::new();
    io.insert("a.wav", f64_wav(FS, &programme(FS, 4.0, 0.01)));
    io.insert("b.wav", f64_wav(48_000, &programme(48_000, 4.0, 0.1)));
    convert(
        &mut io,
        &["a.wav", "b.wav"],
        "ab.wav",
        settings(&["normalize -18LUFS"]),
        1,
    )
    .unwrap();
    let measured = loudness(&mut io, "ab.wav").integrated.unwrap();
    assert!((measured + 18.0).abs() < 0.1, "{measured}");
}

#[test]
fn normalize_after_a_trim_and_after_a_rate_change() {
    let mut io = MemoryIo::new();
    // Loud first second, quiet after: the trim decides what is measured.
    let mut x = programme(FS, 1.0, 0.5);
    x.extend(programme(FS, 6.0, 0.02));
    io.insert("in.wav", f64_wav(FS, &x));
    for (filters, rate) in [
        (&["trim start=1s", "normalize -20LUFS"][..], FS),
        (&["resample 48000", "normalize -20LUFS"][..], 48_000),
        (
            &["trim start=1s", "resample 48kHz", "normalize -20LUFS"][..],
            48_000,
        ),
    ] {
        convert(&mut io, &["in.wav"], "out.wav", settings(filters), 0).unwrap();
        let m = loudness(&mut io, "out.wav");
        assert!(
            (m.integrated.unwrap() + 20.0).abs() < 0.1,
            "{filters:?}: {m:?}"
        );
        let s = &probe("out.wav", &mut io, &Limits::default())
            .unwrap()
            .streams[0];
        assert_eq!(s.audio.sample_rate(), rate);
    }
}

#[test]
fn a_fade_after_normalize_changes_only_the_faded_end() {
    let mut io = MemoryIo::new();
    io.insert("in.wav", f64_wav(FS, &programme(FS, 4.0, 0.03)));
    convert(
        &mut io,
        &["in.wav"],
        "plain.wav",
        settings(&["normalize -20LUFS"]),
        1,
    )
    .unwrap();
    convert(
        &mut io,
        &["in.wav"],
        "faded.wav",
        settings(&["normalize -20LUFS", "fade out=1s"]),
        1,
    )
    .unwrap();
    let read = |io: &MemoryIo, p: &str| {
        pcm::decode_le(common::data_chunk(&io.get(p).unwrap()), SampleFormat::F64).unwrap()
    };
    let (plain, faded) = (read(&io, "plain.wav"), read(&io, "faded.wav"));
    let unfaded = 2 * 3 * FS as usize;
    assert_eq!(
        plain[..unfaded],
        faded[..unfaded],
        "same gain before the fade"
    );
    assert_ne!(plain[unfaded..], faded[unfaded..]);
}

#[test]
fn every_thread_count_gives_the_same_bytes() {
    let mut io = MemoryIo::new();
    io.insert("in.wav", f64_wav(FS, &programme(FS, 4.0, 0.03)));
    let mut outs = Vec::new();
    for threads in [1, 0, 3] {
        let s16 = AudioSettings {
            sample_format: Some(SampleFormat::S16),
            ..settings(&["normalize -16LUFS"])
        };
        convert(&mut io, &["in.wav"], "out.flac", s16, threads).unwrap();
        outs.push(io.get("out.flac").unwrap());
    }
    assert_eq!(outs[0], outs[1]);
    assert_eq!(outs[0], outs[2]);
}

/// Wraps an I/O: counts sinks created, and opens each source at most `opens` times.
struct Guarded {
    inner: MemoryIo,
    opens: HashMap<String, usize>,
    max_opens: usize,
    sinks: usize,
}

impl JobIo for Guarded {
    fn open_source(&mut self, path: &str) -> Result<Box<dyn MediaSource>, Error> {
        let n = self.opens.entry(path.to_owned()).or_default();
        *n += 1;
        if *n > self.max_opens {
            return Err(Error::Io {
                kind: std::io::ErrorKind::Unsupported,
                message: format!("{path} can be read once"),
            });
        }
        self.inner.open_source(path)
    }
    fn create_sink(&mut self, path: &str) -> Result<Box<dyn MediaSink>, Error> {
        self.sinks += 1;
        self.inner.create_sink(path)
    }
    fn commit(&mut self, path: &str) -> Result<(), Error> {
        self.inner.commit(path)
    }
    fn discard(&mut self, path: &str) {
        self.inner.discard(path);
    }
}

fn guarded(max_opens: usize) -> Guarded {
    let mut inner = MemoryIo::new();
    inner.insert("in.wav", f64_wav(FS, &programme(FS, 4.0, 0.03)));
    Guarded {
        inner,
        opens: HashMap::new(),
        max_opens,
        sinks: 0,
    }
}

#[test]
fn a_source_that_cannot_be_reopened_fails_before_any_output_is_written() {
    let mut io = guarded(1);
    let err = convert(
        &mut io,
        &["in.wav"],
        "out.wav",
        settings(&["normalize -16LUFS"]),
        1,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("normalising reads in.wav twice"),
        "{err}"
    );
    assert_eq!(
        io.sinks, 0,
        "the output is only created once the source is open again"
    );
    assert_eq!(io.inner.get("out.wav"), None);
    assert!(!io.inner.has_pending());
}

#[test]
fn cancel_in_either_pass_leaves_nothing() {
    for cancel_after in ["Measuring", "Started"] {
        let mut io = guarded(2);
        let cancel = CancelToken::new();
        let mut armed = false;
        let r = run(
            &job(&["in.wav"], "out.wav", settings(&["normalize -16LUFS"])),
            &mut io,
            &Limits::default(),
            &cancel,
            &mut |e| match e {
                Event::Measuring if cancel_after == "Measuring" => armed = true,
                Event::Started { .. } if cancel_after == "Started" => armed = true,
                Event::Progress { .. } if armed => cancel.cancel(),
                _ => {}
            },
        );
        assert_eq!(r, Err(Error::Cancelled), "{cancel_after}");
        if cancel_after == "Measuring" {
            assert_eq!(io.sinks, 0, "nothing created during the measuring pass");
        }
        assert_eq!(io.inner.get("out.wav"), None);
        assert!(!io.inner.has_pending());
    }
}

#[test]
fn validate_accepts_a_normalising_job_and_pipeline_open_points_at_run() {
    let mut io = MemoryIo::new();
    io.insert("in.wav", f64_wav(FS, &programme(FS, 1.0, 0.03)));
    let stream = &probe("in.wav", &mut io, &Limits::default())
        .unwrap()
        .streams[0];
    let s = settings(&["normalize -16LUFS"]);
    assert!(validate(stream, "o.flac", &s).is_ok());
    assert_eq!(
        describe(stream, "o.flac", &s).unwrap().output.frames,
        Some(44_100)
    );
    let err = Pipeline::open(&job(&["in.wav"], "o.wav", s), &mut io, &Limits::default())
        .err()
        .unwrap();
    assert!(err.to_string().contains("measuring pass"), "{err}");
}
