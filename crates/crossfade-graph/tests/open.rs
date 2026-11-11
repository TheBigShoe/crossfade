mod common;

use common::{job, sine_wav, spec};
use crossfade_core::{ChannelLayout, CodecId, Error, Limits, SampleFormat};
use crossfade_graph::{Job, JobOutput, Pipeline};
use crossfade_testkit::memory::MemoryIo;
use std::io::ErrorKind;

fn io_with_input() -> (MemoryIo, crossfade_core::AudioSpec) {
    let s = spec(8000, SampleFormat::S16, ChannelLayout::stereo());
    let mut io = MemoryIo::new();
    io.insert("in.wav", sine_wav(&s, 100).1);
    (io, s)
}

fn open(job: &Job, io: &mut MemoryIo) -> Result<Pipeline, Error> {
    Pipeline::open(job, io, &Limits::default())
}

fn unsupported_text(job: &Job, io: &mut MemoryIo) -> String {
    match open(job, io) {
        Err(Error::Unsupported { what }) => what,
        other => panic!("expected Unsupported, got {:?}", other.map(|_| ())),
    }
}

#[test]
fn plan_describes_input_and_output() {
    let (mut io, s) = io_with_input();
    let p = open(&job("in.wav", "out.wav", Some(SampleFormat::S24)), &mut io).unwrap();
    let plan = p.plan();
    assert_eq!(plan.input_format.name(), "wav");
    assert_eq!(plan.output_format.name(), "wav");
    assert_eq!(plan.input.audio, s);
    assert_eq!(plan.input.frames, Some(100));
    assert_eq!(plan.output.codec, CodecId::PcmS24Le);
    assert_eq!(plan.output.audio.format(), SampleFormat::S24);
    assert_eq!(plan.output.audio.sample_rate(), 8000);
    assert_eq!(plan.output.audio.layout(), s.layout());
    assert_eq!(plan.output.frames, Some(100));
    assert!(
        io.has_pending(),
        "the sink exists once the pipeline is open"
    );
}

#[test]
fn no_sample_format_keeps_the_input_format() {
    let (mut io, _) = io_with_input();
    let p = open(&job("in.wav", "out.wav", None), &mut io).unwrap();
    assert_eq!(p.plan().output.audio.format(), SampleFormat::S16);
    assert_eq!(p.plan().output.codec, CodecId::PcmS16Le);
}

#[test]
fn output_container_comes_from_the_extension_case_insensitively() {
    let (mut io, _) = io_with_input();
    assert!(open(&job("in.wav", "OUT.WAVE", None), &mut io).is_ok());
}

#[test]
fn at_least_one_input_and_exactly_one_output() {
    let (mut io, _) = io_with_input();
    let base = job("in.wav", "out.wav", None);
    let mut none_in = base.clone();
    none_in.inputs.clear();
    let mut none_out = base.clone();
    none_out.outputs.clear();
    let mut two_out = base.clone();
    two_out.outputs.push(JobOutput {
        path: "b.wav".into(),
        audio: Default::default(),
    });
    for (j, word) in [
        // Several inputs are a join (`crossfade concat`), tested in concat.rs.
        (none_in, "input"),
        (none_out, "output"),
        (two_out, "output"),
    ] {
        let what = unsupported_text(&j, &mut io);
        assert!(what.contains(word), "{what}");
    }
    assert!(!io.has_pending());
}

#[test]
fn unknown_output_extension_is_named() {
    let (mut io, _) = io_with_input();
    let what = unsupported_text(&job("in.wav", "out.mp3", None), &mut io);
    assert!(what.contains("mp3"), "{what}");
    let what = unsupported_text(&job("in.wav", "out", None), &mut io);
    assert!(what.contains("out"), "{what}");
    assert!(!io.has_pending());
}

#[test]
fn other_job_versions_are_unsupported() {
    let (mut io, _) = io_with_input();
    let mut j = job("in.wav", "out.wav", None);
    j.version = 2;
    assert!(unsupported_text(&j, &mut io).contains("version"));
}

#[test]
fn missing_input_is_an_io_error_and_leaves_nothing_pending() {
    let mut io = MemoryIo::new();
    assert!(matches!(
        open(&job("nope.wav", "out.wav", None), &mut io),
        Err(Error::Io {
            kind: ErrorKind::NotFound,
            ..
        })
    ));
    assert!(!io.has_pending());
}

#[test]
fn unrecognised_input_is_reported() {
    let mut io = MemoryIo::new();
    io.insert("in.wav", vec![0; 64]);
    assert_eq!(
        open(&job("in.wav", "out.wav", None), &mut io).err(),
        Some(Error::UnrecognisedFormat)
    );
    assert!(!io.has_pending());
}

#[test]
fn a_sink_that_cannot_take_the_header_is_discarded() {
    let (mut io, _) = io_with_input();
    io.fail_sink_after("out.wav", 10, ErrorKind::StorageFull);
    assert!(matches!(
        open(&job("in.wav", "out.wav", None), &mut io),
        Err(Error::Io {
            kind: ErrorKind::StorageFull,
            ..
        })
    ));
    assert!(!io.has_pending());
}
