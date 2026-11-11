//! A decode-only pass for analysis (loudness, waveforms): every frame of the one audio
//! stream, as `f64`, in order.
mod common;

use common::{job, spec, wav_bytes};
use crossfade_core::{AudioFrame, ChannelLayout, Error, Limits, SampleFormat, Samples};
use crossfade_graph::{CancelToken, decode_frames, run};
use crossfade_testkit::memory::MemoryIo;

fn samples(n: usize) -> Vec<i16> {
    (0..n)
        .map(|i| (((i * 7919) % 65_536) as i32 - 32_768) as i16)
        .collect()
}

fn io_with(n: usize) -> MemoryIo {
    let pcm: Vec<u8> = samples(n).iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut io = MemoryIo::new();
    io.insert(
        "in.wav",
        wav_bytes(
            &spec(8000, SampleFormat::S16, ChannelLayout::stereo()),
            &pcm,
        ),
    );
    io
}

fn collect(io: &mut MemoryIo, path: &str, threads: usize) -> Result<Vec<AudioFrame>, Error> {
    let mut frames = Vec::new();
    let limits = Limits {
        max_threads: threads,
        ..Limits::default()
    };
    let stream = decode_frames(path, io, &limits, &CancelToken::new(), &mut |s, f| {
        assert_eq!(s.audio.sample_rate(), 8000);
        frames.push(f.clone());
        Ok(())
    })?;
    assert_eq!(stream.audio.layout(), &ChannelLayout::stereo());
    Ok(frames)
}

fn joined(frames: &[AudioFrame]) -> Vec<f64> {
    frames
        .iter()
        .flat_map(|f| match &f.samples {
            Samples::F64(v) => v.clone(),
            other => panic!("not f64: {other:?}"),
        })
        .collect()
}

#[test]
fn every_frame_arrives_as_f64_in_order_with_contiguous_pts() {
    let mut io = io_with(20_000);
    let frames = collect(&mut io, "in.wav", 1).unwrap();
    let want: Vec<f64> = samples(20_000)
        .iter()
        .map(|&s| f64::from(s) / 32768.0)
        .collect();
    assert_eq!(joined(&frames), want);
    let mut pts = 0;
    for f in &frames {
        assert_eq!(f.pts, pts);
        pts += f.frames() as i64;
    }
}

#[test]
fn a_flac_decodes_to_the_same_frames_at_any_thread_count() {
    let mut io = io_with(50_000);
    run(
        &job("in.wav", "in.flac", None),
        &mut io,
        &Limits::default(),
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap();
    let wav = joined(&collect(&mut io, "in.wav", 1).unwrap());
    for threads in [1, 0, 3] {
        assert_eq!(joined(&collect(&mut io, "in.flac", threads).unwrap()), wav);
    }
}

#[test]
fn a_cancelled_pass_stops_and_says_so() {
    let mut io = io_with(1000);
    let cancel = CancelToken::new();
    cancel.cancel();
    let mut called = false;
    let r = decode_frames(
        "in.wav",
        &mut io,
        &Limits::default(),
        &cancel,
        &mut |_, _| {
            called = true;
            Ok(())
        },
    );
    assert_eq!(r.err(), Some(Error::Cancelled));
    assert!(!called);
}

#[test]
fn the_callback_can_stop_the_pass_with_an_error() {
    let mut io = io_with(100_000);
    let mut calls = 0;
    let r = decode_frames(
        "in.wav",
        &mut io,
        &Limits::default(),
        &CancelToken::new(),
        &mut |_, _| {
            calls += 1;
            Err(Error::Unsupported {
                what: "enough".into(),
            })
        },
    );
    assert_eq!(calls, 1);
    assert!(r.unwrap_err().to_string().contains("enough"));
}

#[test]
fn a_missing_or_truncated_file_is_an_error() {
    let mut io = io_with(1000);
    assert!(collect(&mut io, "nope.wav", 1).is_err());
    io.truncate_source_at("in.wav", 30);
    assert!(collect(&mut io, "in.wav", 1).is_err());
}
