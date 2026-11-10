mod common;

use common::{data_chunk, job, sine_wav, spec};
use crossfade_core::{ChannelLayout, Error, JobIo, Limits, MediaSink, MediaSource, SampleFormat};
use crossfade_graph::{CancelToken, Event, Job, run};
use crossfade_testkit::memory::MemoryIo;
use std::io::ErrorKind;

const FRAMES: usize = 10_000;

fn io_with_input() -> (MemoryIo, Vec<u8>) {
    let s = spec(8000, SampleFormat::S16, ChannelLayout::mono());
    let wav = sine_wav(&s, FRAMES).1;
    let mut io = MemoryIo::new();
    io.insert("in.wav", wav.clone());
    (io, wav)
}

fn convert_job() -> Job {
    job("in.wav", "out.wav", Some(SampleFormat::S24))
}

fn run_collect(
    job: &Job,
    io: &mut dyn JobIo,
    cancel: &CancelToken,
) -> (Result<(), Error>, Vec<Event>) {
    let mut events = Vec::new();
    let result = run(job, io, &Limits::default(), cancel, &mut |e| {
        events.push(e.clone());
    });
    (result, events)
}

fn assert_nothing_written(io: &MemoryIo) {
    assert_eq!(io.get("out.wav"), None);
    assert!(!io.has_pending());
}

#[test]
fn token_clones_share_the_flag() {
    let a = CancelToken::new();
    let b = a.clone();
    assert!(!b.is_cancelled());
    a.cancel();
    assert!(b.is_cancelled());
    assert!(!CancelToken::default().is_cancelled());
}

#[test]
fn events_arrive_in_order_and_the_output_is_committed() {
    let (mut io, wav) = io_with_input();
    let (result, events) = run_collect(&convert_job(), &mut io, &CancelToken::new());
    result.unwrap();

    let Some(Event::Started { plan }) = events.first() else {
        panic!("first event: {:?}", events.first());
    };
    assert_eq!(plan.input.frames, Some(FRAMES as u64));
    assert_eq!(
        events.last(),
        Some(&Event::Finished {
            frames: FRAMES as u64
        })
    );
    let progress: Vec<u64> = events[1..events.len() - 1]
        .iter()
        .map(|e| match e {
            Event::Progress {
                frames_done,
                frames_total,
            } => {
                assert_eq!(*frames_total, Some(FRAMES as u64));
                *frames_done
            }
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(progress.len(), 3);
    assert!(progress.windows(2).all(|w| w[0] <= w[1]));
    assert_eq!(progress.last(), Some(&(FRAMES as u64)));

    assert!(!io.has_pending());
    let out = io.get("out.wav").unwrap();
    assert_eq!(data_chunk(&out).len(), data_chunk(&wav).len() / 2 * 3);
}

#[test]
fn cancel_before_the_first_step() {
    let (mut io, _) = io_with_input();
    let token = CancelToken::new();
    token.cancel();
    let (result, events) = run_collect(&convert_job(), &mut io, &token);
    assert_eq!(result, Err(Error::Cancelled));
    assert!(matches!(events.as_slice(), [Event::Started { .. }]));
    assert_nothing_written(&io);
}

#[test]
fn cancel_from_the_callback_after_the_first_progress() {
    let (mut io, _) = io_with_input();
    let token = CancelToken::new();
    let mut events = Vec::new();
    let result = run(
        &convert_job(),
        &mut io,
        &Limits::default(),
        &token,
        &mut |e| {
            if matches!(e, Event::Progress { .. }) {
                token.cancel();
            }
            events.push(e.clone());
        },
    );
    assert_eq!(result, Err(Error::Cancelled));
    let progress = events
        .iter()
        .filter(|e| matches!(e, Event::Progress { .. }))
        .count();
    assert_eq!(progress, 1);
    assert!(!events.iter().any(|e| matches!(e, Event::Finished { .. })));
    assert_nothing_written(&io);
}

#[test]
fn truncated_source_is_unexpected_eof() {
    let (mut io, _) = io_with_input();
    io.truncate_source_at("in.wav", 44 + 10_000);
    let (result, events) = run_collect(&convert_job(), &mut io, &CancelToken::new());
    assert!(
        matches!(result, Err(Error::UnexpectedEof { .. })),
        "{result:?}"
    );
    assert!(!events.iter().any(|e| matches!(e, Event::Finished { .. })));
    assert_nothing_written(&io);
}

#[test]
fn failing_source_is_an_io_error() {
    let (mut io, _) = io_with_input();
    io.fail_source_at("in.wav", 44 + 10_000, ErrorKind::PermissionDenied);
    let (result, _) = run_collect(&convert_job(), &mut io, &CancelToken::new());
    assert!(
        matches!(
            result,
            Err(Error::Io {
                kind: ErrorKind::PermissionDenied,
                ..
            })
        ),
        "{result:?}"
    );
    assert_nothing_written(&io);
}

#[test]
fn failing_sink_is_an_io_error() {
    let (mut io, _) = io_with_input();
    io.fail_sink_after("out.wav", 5_000, ErrorKind::StorageFull);
    let (result, _) = run_collect(&convert_job(), &mut io, &CancelToken::new());
    assert!(
        matches!(
            result,
            Err(Error::Io {
                kind: ErrorKind::StorageFull,
                ..
            })
        ),
        "{result:?}"
    );
    assert_nothing_written(&io);
}

#[test]
fn a_rejected_job_emits_nothing() {
    let (mut io, _) = io_with_input();
    let (result, events) = run_collect(
        &job("in.wav", "out.xyz", None),
        &mut io,
        &CancelToken::new(),
    );
    assert!(matches!(result, Err(Error::Unsupported { .. })));
    assert!(events.is_empty());
    assert!(!io.has_pending());
}

/// Fails the final rename-into-place step.
struct NoCommit(MemoryIo);

impl JobIo for NoCommit {
    fn open_source(&mut self, path: &str) -> Result<Box<dyn MediaSource>, Error> {
        self.0.open_source(path)
    }
    fn create_sink(&mut self, path: &str) -> Result<Box<dyn MediaSink>, Error> {
        self.0.create_sink(path)
    }
    fn commit(&mut self, _: &str) -> Result<(), Error> {
        Err(Error::Io {
            kind: ErrorKind::PermissionDenied,
            message: "rename".into(),
        })
    }
    fn discard(&mut self, path: &str) {
        self.0.discard(path);
    }
}

#[test]
fn a_failed_commit_discards_the_partial_output() {
    let (io, _) = io_with_input();
    let mut io = NoCommit(io);
    let (result, events) = run_collect(&convert_job(), &mut io, &CancelToken::new());
    assert!(matches!(
        result,
        Err(Error::Io {
            kind: ErrorKind::PermissionDenied,
            ..
        })
    ));
    assert!(!events.iter().any(|e| matches!(e, Event::Finished { .. })));
    assert_nothing_written(&io.0);
}
