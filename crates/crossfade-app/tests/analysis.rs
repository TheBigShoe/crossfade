mod common;

use common::{spec, wav_bytes};
use crossfade_app::analysis::{Analysis, Analyzer, State, analyze, roles_for};
use crossfade_app::waveform::Pyramid;
use crossfade_core::{AudioFrame, ChannelLayout, Limits, SampleFormat, Samples};
use crossfade_graph::{CancelToken, decode_frames};
use crossfade_loudness::{ChannelRole, Meter, Options};
use crossfade_testkit::memory::MemoryIo;
use std::sync::{Arc, Mutex};

const RATE: u32 = 8000;

/// A loud two-channel tone with a quiet second half and a different level per channel.
fn pcm(frames: usize) -> Vec<u8> {
    (0..frames)
        .flat_map(|i| {
            let t = i as f64 / f64::from(RATE);
            let level = if i < frames / 2 { 0.5 } else { 0.05 };
            let l = (level * (t * 2.0 * std::f64::consts::PI * 440.0).sin() * 32767.0) as i16;
            let r = (f64::from(l) * 0.5) as i16;
            [l.to_le_bytes(), r.to_le_bytes()].concat()
        })
        .collect()
}

fn io_with(frames: usize) -> MemoryIo {
    let mut io = MemoryIo::new();
    io.insert(
        "in.wav",
        wav_bytes(
            &spec(RATE, SampleFormat::S16, ChannelLayout::stereo()),
            &pcm(frames),
        ),
    );
    io
}

fn decoded(io: &mut MemoryIo) -> (Vec<AudioFrame>, usize) {
    let mut frames = Vec::new();
    decode_frames(
        "in.wav",
        io,
        &Limits::default(),
        &CancelToken::new(),
        &mut |_, f| {
            frames.push(f.clone());
            Ok(())
        },
    )
    .unwrap();
    let biggest = frames.iter().map(AudioFrame::frames).max().unwrap_or(0);
    (frames, biggest)
}

fn run(io: MemoryIo) -> Analysis {
    let a = Analyzer::start("in.wav", Box::new(io), Limits::default(), |_| {});
    let snap = a.wait();
    assert_eq!(snap.state, State::Done);
    a.with(Analysis::clone)
}

#[test]
fn a_background_run_equals_the_single_threaded_pass() {
    let frames = 8000 * 20;
    let single = analyze(
        "in.wav",
        &mut io_with(frames),
        &Limits::default(),
        &CancelToken::new(),
    );
    assert_eq!(single.state, State::Done);
    assert_eq!(single.frames_done, frames as u64);
    assert_eq!(run(io_with(frames)), single);
}

#[test]
fn the_pyramid_and_loudness_match_the_decoded_samples() {
    let n = 8000 * 20;
    let mut io = io_with(n);
    let (frames, _) = decoded(&mut io);
    let mut pyramid = Pyramid::new(2);
    let mut meter = Meter::new(
        RATE,
        &[ChannelRole::Left, ChannelRole::Right],
        Options::default(),
    )
    .unwrap();
    let mut curve = Vec::new();
    let mut fed = 0usize;
    for f in &frames {
        pyramid.push(f).unwrap();
        let Samples::F64(s) = &f.samples else {
            panic!()
        };
        // The curve is read every 100 ms of programme (800 frames here).
        for pair in s.chunks(2) {
            meter.push(pair);
            fed += 1;
            if fed.is_multiple_of(800) {
                curve.push(meter.short_term().map_or(f32::NEG_INFINITY, |v| v as f32));
            }
        }
    }
    let m = meter.finish();
    let a = run(io);
    assert_eq!(a.pyramid, pyramid);
    assert_eq!(a.loudness.integrated, m.integrated);
    assert_eq!(a.loudness.range, m.loudness_range);
    assert_eq!(a.loudness.true_peak, m.true_peak);
    assert_eq!(a.loudness.sample_peak, m.sample_peak);
    assert!(a.loudness.integrated.is_some());
    assert_eq!(a.short_term, curve);
    assert_eq!(a.short_term.len(), n / 800);
    assert!(a.short_term[..29].iter().all(|v| *v == f32::NEG_INFINITY));
    assert!(a.short_term[30].is_finite());
    assert!(a.short_term.iter().all(|v| !v.is_nan()));
    let s = a.stream.unwrap();
    assert_eq!((s.audio.sample_rate(), s.frames), (RATE, Some(n as u64)));
}

#[test]
fn progress_is_reported_by_frame_count_and_ends_with_done() {
    let n = 8000 * 20;
    let (_, biggest) = decoded(&mut io_with(n));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let a = Analyzer::start(
        "in.wav",
        Box::new(io_with(n)),
        Limits::default(),
        move |s| {
            sink.lock().unwrap().push((s.frames_done, s.state.clone()));
        },
    );
    a.wait();
    // The terminal notification is sent after the state is final; wait for it by polling the
    // recorded list through a final snapshot barrier.
    drop(a);
    let seen = seen.lock().unwrap();
    let (last_done, last_state) = seen.last().unwrap();
    assert_eq!((*last_done, last_state), (n as u64, &State::Done));
    let step = n as u64 / 200;
    let mut prev = 0;
    for (done, _) in seen.iter() {
        assert!(done - prev <= step + biggest as u64, "{prev} -> {done}");
        assert!(*done >= prev);
        prev = *done;
    }
    assert!(seen.len() >= 10);
}

#[test]
fn cancelling_mid_file_stops_with_a_drawable_prefix() {
    let n = 8000 * 20;
    let token = CancelToken::new();
    let t2 = token.clone();
    let a = Analyzer::start_with_cancel(
        "in.wav",
        Box::new(io_with(n)),
        Limits::default(),
        token,
        move |s| {
            if s.frames_done > 0 {
                t2.cancel();
            }
        },
    );
    let snap = a.wait();
    assert_eq!(snap.state, State::Cancelled);
    assert!(snap.frames_done > 0 && snap.frames_done < n as u64);
    a.with(|an| {
        assert_eq!(an.pyramid.frames(), an.frames_done);
        assert!(an.pyramid.columns(0, 0, 100, 3)[0].is_some());
    });
}

#[test]
fn cancel_is_a_method_and_dropping_cancels() {
    let token = CancelToken::new();
    let a = Analyzer::start_with_cancel(
        "in.wav",
        Box::new(io_with(8000 * 20)),
        Limits::default(),
        token.clone(),
        |_| {},
    );
    drop(a);
    assert!(token.is_cancelled());
    let a = Analyzer::start(
        "in.wav",
        Box::new(io_with(8000 * 20)),
        Limits::default(),
        |_| {},
    );
    a.cancel();
    let s = a.wait();
    assert!(matches!(s.state, State::Cancelled | State::Done));
}

#[test]
fn a_missing_or_bad_file_fails_with_the_engines_message() {
    let a = Analyzer::start(
        "nope.wav",
        Box::new(MemoryIo::new()),
        Limits::default(),
        |_| {},
    );
    let State::Failed(msg) = a.wait().state else {
        panic!("not failed")
    };
    assert!(msg.contains("nope.wav"), "{msg}");
    let mut io = MemoryIo::new();
    io.insert("in.wav", vec![0x42; 200]);
    let a = Analyzer::start("in.wav", Box::new(io), Limits::default(), |_| {});
    assert!(matches!(a.wait().state, State::Failed(m) if !m.is_empty()));
}

#[test]
fn a_rate_the_meter_cannot_take_still_gets_a_waveform() {
    let mut io = MemoryIo::new();
    io.insert(
        "in.wav",
        wav_bytes(
            &spec(4000, SampleFormat::S16, ChannelLayout::mono()),
            &[1, 2, 3, 4, 5, 6, 7, 8],
        ),
    );
    let a = run(io);
    assert_eq!(a.loudness.integrated, None);
    assert_eq!(a.loudness.true_peak, None);
    assert_eq!(a.pyramid.frames(), 4);
}

#[test]
fn an_empty_file_is_done_with_nothing() {
    let mut io = MemoryIo::new();
    io.insert(
        "in.wav",
        wav_bytes(&spec(RATE, SampleFormat::S16, ChannelLayout::stereo()), &[]),
    );
    let a = run(io);
    assert_eq!((a.frames_done, a.pyramid.frames()), (0, 0));
    assert!(a.short_term.is_empty());
}

#[test]
fn roles_follow_the_layout() {
    use ChannelRole::*;
    assert_eq!(roles_for(&ChannelLayout::stereo()), [Left, Right]);
    assert_eq!(roles_for(&ChannelLayout::mono()), [Unweighted]);
    let l51 = ChannelLayout::from_name("5.1").unwrap();
    assert_eq!(
        roles_for(&l51),
        [Left, Right, Centre, Lfe, LeftSurround, RightSurround]
    );
    assert_eq!(
        roles_for(&ChannelLayout::unpositioned(3).unwrap()),
        [Left, Right, Centre]
    );
    assert_eq!(
        roles_for(&ChannelLayout::unpositioned(9).unwrap()),
        [Unweighted; 9]
    );
}

/// By hand: `CROSSFADE_ANALYSIS_FILE=big.wav cargo test --release -p crossfade-app --test
/// analysis -- --ignored --nocapture`. Prints the time; asserts nothing about it.
#[test]
#[ignore]
fn time_one_real_file() {
    let path = std::env::var("CROSSFADE_ANALYSIS_FILE").expect("set CROSSFADE_ANALYSIS_FILE");
    let started = std::time::Instant::now();
    let a = analyze(
        &path,
        &mut crossfade_app::FsIo::new(),
        &Limits::default(),
        &CancelToken::new(),
    );
    println!(
        "{:?}: {} frames in {:?}, pyramid {} bytes, integrated {:?}",
        a.state,
        a.frames_done,
        started.elapsed(),
        a.pyramid.heap_bytes(),
        a.loudness.integrated
    );
    assert_eq!(a.state, State::Done);
}
