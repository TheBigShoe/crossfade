//! Joining inputs end to end (`crossfade concat`): one input open at a time, each conformed
//! to the joined stream's rate and layout, sample-exact seams, one progress from 0 to the
//! sum, nothing left behind on failure or cancel.
mod common;

use common::{data_chunk, spec, wav_bytes};
use crossfade_core::{ChannelLayout, Error, JobIo, Limits, SampleFormat};
use crossfade_graph::{
    AudioSettings, CancelToken, Dither, Event, Job, JobInput, JobOutput, Pipeline, Step, probe,
    run, validate_concat,
};
use crossfade_resample::{Quality, Resampler};
use crossfade_testkit::memory::MemoryIo;
use crossfade_testkit::pcm;
use std::io::ErrorKind;

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

fn join(
    io: &mut MemoryIo,
    inputs: &[&str],
    output: &str,
    audio: AudioSettings,
) -> Result<Vec<Event>, Error> {
    join_threads(io, inputs, output, audio, 1)
}

fn join_threads(
    io: &mut MemoryIo,
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

/// Distinct values per input, so a seam shows exactly where each sample came from.
fn ramp(n: usize, channels: usize, from: f64) -> Vec<f64> {
    (0..n * channels)
        .map(|i| (from + i as f64 * 1e-4).sin() * 0.8)
        .collect()
}

fn wav(rate: u32, format: SampleFormat, layout: ChannelLayout, x: &[f64]) -> Vec<u8> {
    wav_bytes(&spec(rate, format, layout), &pcm::encode_le(x, format))
}

fn stereo() -> ChannelLayout {
    ChannelLayout::stereo()
}

fn f64_out() -> AudioSettings {
    AudioSettings {
        sample_format: Some(SampleFormat::F64),
        ..AudioSettings::default()
    }
}

fn f64s(bytes: &[u8]) -> Vec<f64> {
    pcm::decode_le(data_chunk(bytes), SampleFormat::F64).unwrap()
}

#[test]
fn inputs_alike_join_bit_exactly_with_no_dither() {
    let (a, b, c) = (ramp(1000, 2, 0.0), ramp(777, 2, 1.0), ramp(5, 2, 2.0));
    let mut io = MemoryIo::new();
    for (name, x) in [("a.wav", &a), ("b.wav", &b), ("c.wav", &c)] {
        io.insert(name, wav(44_100, SampleFormat::S16, stereo(), x));
    }
    join(
        &mut io,
        &["a.wav", "b.wav", "c.wav"],
        "abc.wav",
        AudioSettings::default(),
    )
    .unwrap();
    let mut want = data_chunk(&io.get("a.wav").unwrap()).to_vec();
    want.extend_from_slice(data_chunk(&io.get("b.wav").unwrap()));
    want.extend_from_slice(data_chunk(&io.get("c.wav").unwrap()));
    assert_eq!(data_chunk(&io.get("abc.wav").unwrap()), want.as_slice());
    // Through FLAC too, read back by the engine (which checks the MD5).
    join(
        &mut io,
        &["a.wav", "b.wav", "c.wav"],
        "abc.flac",
        AudioSettings::default(),
    )
    .unwrap();
    join(&mut io, &["abc.flac"], "back.wav", AudioSettings::default()).unwrap();
    assert_eq!(data_chunk(&io.get("back.wav").unwrap()), want.as_slice());
}

#[test]
fn a_second_rate_is_conformed_and_the_seam_is_sample_exact() {
    let (a, b) = (ramp(3000, 2, 0.0), ramp(4000, 2, 3.0));
    let mut io = MemoryIo::new();
    io.insert("a.wav", wav(44_100, SampleFormat::F64, stereo(), &a));
    io.insert("b.wav", wav(48_000, SampleFormat::F64, stereo(), &b));
    join(&mut io, &["a.wav", "b.wav"], "ab.wav", f64_out()).unwrap();
    let out = f64s(&io.get("ab.wav").unwrap());
    let mut r = Resampler::new(48_000, 44_100, 2, Quality::Balanced).unwrap();
    let mut b44 = Vec::new();
    r.process(&b, &mut b44).unwrap();
    r.flush(&mut b44);
    assert_eq!(b44.len(), 2 * 3675, "ceil(4000 * 44100 / 48000)");
    assert_eq!(out.len(), a.len() + b44.len(), "no gap, no extra frame");
    assert_eq!(
        &out[..a.len()],
        a.as_slice(),
        "the first input is untouched"
    );
    assert_eq!(
        &out[a.len()..],
        b44.as_slice(),
        "the second is exactly its own conversion"
    );
    let stream = &probe("ab.wav", &mut io, &Limits::default())
        .unwrap()
        .streams[0];
    assert_eq!(stream.audio.sample_rate(), 44_100);
}

#[test]
fn the_output_settings_set_the_common_rate_for_every_input() {
    let (a, b) = (ramp(441, 1, 0.0), ramp(480, 1, 1.0));
    let mut io = MemoryIo::new();
    io.insert(
        "a.wav",
        wav(44_100, SampleFormat::F64, ChannelLayout::mono(), &a),
    );
    io.insert(
        "b.wav",
        wav(48_000, SampleFormat::F64, ChannelLayout::mono(), &b),
    );
    let at_48k = AudioSettings {
        sample_rate: Some(48_000),
        ..f64_out()
    };
    join(&mut io, &["a.wav", "b.wav"], "ab.wav", at_48k).unwrap();
    let out = f64s(&io.get("ab.wav").unwrap());
    assert_eq!(out.len(), 480 + 480);
    assert_eq!(
        &out[480..],
        b.as_slice(),
        "already at the common rate: untouched"
    );
}

#[test]
fn formats_that_differ_take_the_first_inputs_or_the_one_asked_for() {
    let (a, b) = (ramp(500, 2, 0.0), ramp(500, 2, 1.0));
    let mut io = MemoryIo::new();
    io.insert("a16.wav", wav(48_000, SampleFormat::S16, stereo(), &a));
    io.insert("b24.wav", wav(48_000, SampleFormat::S24, stereo(), &b));
    // Widening to s24: exact, no dither.
    let s24 = AudioSettings {
        sample_format: Some(SampleFormat::S24),
        ..AudioSettings::default()
    };
    join(&mut io, &["a16.wav", "b24.wav"], "w.wav", s24).unwrap();
    let w = pcm::decode_le(data_chunk(&io.get("w.wav").unwrap()), SampleFormat::S24).unwrap();
    let a16 = pcm::decode_le(data_chunk(&io.get("a16.wav").unwrap()), SampleFormat::S16).unwrap();
    let b24 = pcm::decode_le(data_chunk(&io.get("b24.wav").unwrap()), SampleFormat::S24).unwrap();
    assert_eq!(&w[..1000], a16.as_slice());
    assert_eq!(&w[1000..], b24.as_slice());
    // The first input's s16: the s24 input loses real bits, so the join is dithered...
    join(
        &mut io,
        &["a16.wav", "b24.wav"],
        "n.wav",
        AudioSettings::default(),
    )
    .unwrap();
    let n = io.get("n.wav").unwrap();
    assert_eq!(
        probe("n.wav", &mut io, &Limits::default()).unwrap().streams[0]
            .audio
            .format(),
        SampleFormat::S16
    );
    // ...unless the job says none: then the s16 part is untouched and the s24 part truncated.
    let none = AudioSettings {
        dither: Dither::None,
        ..AudioSettings::default()
    };
    join(&mut io, &["a16.wav", "b24.wav"], "t.wav", none).unwrap();
    let t = io.get("t.wav").unwrap();
    assert_ne!(n, t);
    let t = data_chunk(&t);
    assert_eq!(&t[..2000], data_chunk(&io.get("a16.wav").unwrap()));
    let truncated: Vec<u8> = data_chunk(&io.get("b24.wav").unwrap())
        .as_chunks::<3>()
        .0
        .iter()
        .flat_map(|s| [s[1], s[2]])
        .collect();
    assert_eq!(&t[2000..], truncated.as_slice());
}

#[test]
fn filters_run_on_the_joined_stream_and_tags_come_from_the_first_input() {
    let a = ramp(100, 2, 0.0);
    let mut first = wav(48_000, SampleFormat::F64, stereo(), &a);
    // A LIST/INFO chunk with a title, after the data.
    let mut info = b"INFO".to_vec();
    info.extend_from_slice(b"INAM");
    info.extend_from_slice(&6u32.to_le_bytes());
    info.extend_from_slice(b"First\0");
    first.extend_from_slice(b"LIST");
    first.extend_from_slice(&(info.len() as u32).to_le_bytes());
    first.extend_from_slice(&info);
    let riff = (first.len() - 8) as u32;
    first[4..8].copy_from_slice(&riff.to_le_bytes());
    let mut io = MemoryIo::new();
    io.insert("a.wav", first);
    io.insert("b.wav", wav(48_000, SampleFormat::F64, stereo(), &a));
    let half = AudioSettings {
        filters: vec!["gain -20dB".parse().unwrap()],
        ..f64_out()
    };
    join(&mut io, &["a.wav", "b.wav"], "ab.wav", half).unwrap();
    let out = f64s(&io.get("ab.wav").unwrap());
    let k = crossfade_dsp::level::db_to_linear(-20.0);
    let want: Vec<f64> = a.iter().chain(&a).map(|x| x * k).collect();
    assert_eq!(out, want);
    let report = probe("ab.wav", &mut io, &Limits::default()).unwrap();
    let title: Vec<&str> = report.tags.iter().map(|t| t.value.as_str()).collect();
    assert_eq!(title, ["First"]);
}

#[test]
fn progress_runs_once_from_zero_to_the_sum_of_the_inputs() {
    let mut io = MemoryIo::new();
    io.insert(
        "a.wav",
        wav(8000, SampleFormat::S16, stereo(), &ramp(30_000, 2, 0.0)),
    );
    io.insert(
        "b.wav",
        wav(16_000, SampleFormat::S16, stereo(), &ramp(20_000, 2, 0.0)),
    );
    let events = join(
        &mut io,
        &["a.wav", "b.wav"],
        "ab.flac",
        AudioSettings::default(),
    )
    .unwrap();
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
    assert!(progress.len() > 4);
    assert!(progress.iter().all(|p| p.1 == Some(50_000)));
    assert!(
        progress.windows(2).all(|w| w[0].0 <= w[1].0),
        "never goes back"
    );
    assert_eq!(progress.last().unwrap().0, 50_000);
    assert_eq!(events.last(), Some(&Event::Finished { frames: 50_000 }));
    // 30,000 frames at 8 kHz, then 20,000 at 16 kHz made 10,000 at 8 kHz.
    let s = &probe("ab.flac", &mut io, &Limits::default())
        .unwrap()
        .streams[0];
    assert_eq!(s.frames, Some(40_000));
}

#[test]
fn an_input_failing_half_way_leaves_nothing_behind() {
    let mut io = MemoryIo::new();
    io.insert(
        "a.wav",
        wav(8000, SampleFormat::S16, stereo(), &ramp(1000, 2, 0.0)),
    );
    io.insert(
        "b.wav",
        wav(8000, SampleFormat::S16, stereo(), &ramp(100_000, 2, 0.0)),
    );
    io.fail_source_at("b.wav", 50_000, ErrorKind::Other);
    let err = join(
        &mut io,
        &["a.wav", "b.wav"],
        "ab.wav",
        AudioSettings::default(),
    );
    assert!(err.is_err());
    assert_eq!(io.get("ab.wav"), None);
    assert!(!io.has_pending());
    // A missing input is found before anything is created.
    let err = join(
        &mut io,
        &["a.wav", "nope.wav"],
        "ab.wav",
        AudioSettings::default(),
    );
    assert!(err.is_err());
    assert!(!io.has_pending());
}

#[test]
fn cancelling_between_inputs_leaves_nothing_behind() {
    let mut io = MemoryIo::new();
    io.insert(
        "a.wav",
        wav(8000, SampleFormat::S16, stereo(), &ramp(1000, 2, 0.0)),
    );
    io.insert(
        "b.wav",
        wav(8000, SampleFormat::S16, stereo(), &ramp(100_000, 2, 0.0)),
    );
    let cancel = CancelToken::new();
    let r = run(
        &job(&["a.wav", "b.wav"], "ab.wav", AudioSettings::default()),
        &mut io,
        &Limits::default(),
        &cancel,
        &mut |e| {
            if let Event::Progress { frames_done, .. } = e
                && *frames_done > 1000
            {
                cancel.cancel();
            }
        },
    );
    assert_eq!(r, Err(Error::Cancelled));
    assert_eq!(io.get("ab.wav"), None);
    assert!(!io.has_pending());
}

#[test]
fn thread_count_never_changes_a_join() {
    let mut io = MemoryIo::new();
    io.insert(
        "a.wav",
        wav(44_100, SampleFormat::S16, stereo(), &ramp(50_000, 2, 0.0)),
    );
    io.insert(
        "b.wav",
        wav(48_000, SampleFormat::S24, stereo(), &ramp(50_000, 2, 1.0)),
    );
    join(&mut io, &["a.wav"], "a.flac", AudioSettings::default()).unwrap();
    let mut outs = Vec::new();
    for threads in [1, 0, 3] {
        join_threads(
            &mut io,
            &["a.flac", "b.wav", "a.flac"],
            "o.flac",
            AudioSettings::default(),
            threads,
        )
        .unwrap();
        outs.push(io.get("o.flac").unwrap());
    }
    assert_eq!(outs[0], outs[1]);
    assert_eq!(outs[0], outs[2]);
}

/// Mono into a stereo join: the mono input is mixed to both sides at unity.
#[test]
fn a_mono_input_joins_a_stereo_one_on_both_sides() {
    let a = ramp(100, 2, 0.0);
    let m = ramp(100, 1, 0.5);
    let mut io = MemoryIo::new();
    io.insert("a.wav", wav(48_000, SampleFormat::F64, stereo(), &a));
    io.insert(
        "m.wav",
        wav(48_000, SampleFormat::F64, ChannelLayout::mono(), &m),
    );
    join(&mut io, &["a.wav", "m.wav"], "am.wav", f64_out()).unwrap();
    let out = f64s(&io.get("am.wav").unwrap());
    let mut want = a.clone();
    want.extend(m.iter().flat_map(|&x| [x, x]));
    assert_eq!(out, want);
    let s = &probe("am.wav", &mut io, &Limits::default())
        .unwrap()
        .streams[0];
    assert_eq!(s.audio.layout(), &stereo());
}

#[test]
fn validate_concat_agrees_with_the_pipeline() {
    let mut io = MemoryIo::new();
    io.insert(
        "a.wav",
        wav(8000, SampleFormat::S16, stereo(), &ramp(10, 2, 0.0)),
    );
    io.insert(
        "b.wav",
        wav(16_000, SampleFormat::S16, stereo(), &ramp(10, 2, 0.0)),
    );
    let streams: Vec<_> = ["a.wav", "b.wav"]
        .iter()
        .map(|p| probe(p, &mut io, &Limits::default()).unwrap().streams[0].clone())
        .collect();
    let plan = validate_concat(&streams, "o.flac", &AudioSettings::default()).unwrap();
    assert_eq!(plan.output.frames, Some(15));
    let mut p = Pipeline::open(
        &job(&["a.wav", "b.wav"], "o.flac", AudioSettings::default()),
        &mut io,
        &Limits::default(),
    )
    .unwrap();
    assert_eq!(p.plan().output, plan.output);
    // Stepping across inputs needs the I/O to open the next one.
    let mut steps = 0;
    loop {
        match p.step_with(&mut io).unwrap() {
            Step::Finished => break,
            Step::Progress { .. } => steps += 1,
        }
    }
    assert!(steps >= 2);
    io.commit("o.flac").unwrap();
}
