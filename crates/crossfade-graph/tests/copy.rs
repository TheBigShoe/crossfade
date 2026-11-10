mod common;

use common::{data_chunk, job, sine_wav, spec};
use crossfade_core::{ChannelLayout, Error, JobIo, Limits, SampleFormat};
use crossfade_graph::{CancelToken, Event, Job, Pipeline, run};
use crossfade_testkit::memory::MemoryIo;

fn with_audio(mut j: Job, f: impl FnOnce(&mut crossfade_graph::AudioSettings)) -> Job {
    f(&mut j.outputs[0].audio);
    j
}

fn io_with_input(frames: usize) -> (MemoryIo, Vec<u8>) {
    let s = spec(8000, SampleFormat::S16, ChannelLayout::stereo());
    let wav = sine_wav(&s, frames).1;
    let mut io = MemoryIo::new();
    io.insert("in.wav", wav.clone());
    (io, wav)
}

#[test]
fn wav_to_wav_copy_keeps_the_data_chunk_and_reports_frames() {
    let (mut io, wav) = io_with_input(10_000);
    let j = with_audio(job("in.wav", "out.wav", None), |a| {
        a.codec = Some("copy".into())
    });
    let mut events = Vec::new();
    run(
        &j,
        &mut io,
        &Limits::default(),
        &CancelToken::new(),
        &mut |e| {
            events.push(e.clone());
        },
    )
    .unwrap();
    let out = io.get("out.wav").unwrap();
    assert_eq!(data_chunk(&out), data_chunk(&wav));
    let Some(Event::Started { plan }) = events.first() else {
        panic!("no start");
    };
    assert!(plan.copy);
    assert_eq!(events.last(), Some(&Event::Finished { frames: 10_000 }));
}

#[test]
fn a_normal_conversion_is_not_a_copy() {
    let (mut io, _) = io_with_input(100);
    let p = Pipeline::open(&job("in.wav", "out.wav", None), &mut io, &Limits::default()).unwrap();
    assert!(!p.plan().copy);
}

#[test]
fn copy_with_a_sample_format_is_refused_and_leaves_no_output() {
    let (mut io, _) = io_with_input(100);
    let j = with_audio(job("in.wav", "out.wav", Some(SampleFormat::S24)), |a| {
        a.codec = Some("copy".into());
    });
    let r = Pipeline::open(&j, &mut io, &Limits::default());
    assert!(matches!(r.err(), Some(Error::Unsupported { .. })));
    assert!(!io.has_pending());
    assert_eq!(io.get("out.wav"), None);
}

#[test]
fn a_codec_the_container_cannot_hold_is_refused() {
    let (mut io, _) = io_with_input(100);
    let j = with_audio(job("in.wav", "out.wav", None), |a| {
        a.codec = Some("flac".into())
    });
    let r = Pipeline::open(&j, &mut io, &Limits::default());
    assert!(matches!(r.err(), Some(Error::Unsupported { .. })));
    assert!(!io.has_pending());
}

#[test]
fn a_level_with_a_pcm_output_is_refused() {
    let (mut io, _) = io_with_input(100);
    let j = with_audio(job("in.wav", "out.wav", None), |a| {
        a.compression_level = Some(3)
    });
    assert!(matches!(
        Pipeline::open(&j, &mut io, &Limits::default()).err(),
        Some(Error::Unsupported { .. })
    ));
}
