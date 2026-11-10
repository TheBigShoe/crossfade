mod common;

use common::{data_chunk, sine_wav, spec, wav_bytes};
use crossfade_app::Request;
use crossfade_app::queue::{JobId, Queue, Runner, RunnerEvent};
use crossfade_app::runner::ThreadRunner;
use crossfade_core::{ChannelLayout, Limits, SampleFormat};
use crossfade_graph::{AudioSettings, CancelToken, Job, JobInput, JobOutput};
use crossfade_testkit::pcm;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("crossfade-app-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }

    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }

    fn write(&self, name: &str, bytes: &[u8]) {
        std::fs::write(self.0.join(name), bytes).unwrap();
    }

    fn listing(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(&self.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn request(input: &str, output: &str, format: Option<SampleFormat>, overwrite: bool) -> Request {
    Request {
        job: Job {
            version: Job::VERSION,
            inputs: vec![JobInput { path: input.into() }],
            outputs: vec![JobOutput {
                path: output.into(),
                audio: AudioSettings {
                    sample_format: format,
                    ..AudioSettings::default()
                },
            }],
        },
        overwrite,
    }
}

fn job_id() -> JobId {
    Queue::new().enqueue(request("x", "y", None, false))
}

fn is_terminal(e: &RunnerEvent) -> bool {
    matches!(
        e,
        RunnerEvent::Finished { .. } | RunnerEvent::Failed { .. } | RunnerEvent::Cancelled
    )
}

/// All events up to and including the terminal one. The timeout only bounds a
/// hang; nothing asserts on elapsed time.
fn collect(rx: &Receiver<(JobId, RunnerEvent)>, id: JobId) -> Vec<RunnerEvent> {
    let mut events = Vec::new();
    loop {
        let (got, event) = rx
            .recv_timeout(Duration::from_secs(20))
            .expect("terminal event");
        assert_eq!(got, id);
        let done = is_terminal(&event);
        events.push(event);
        if done {
            return events;
        }
    }
}

fn runner() -> (ThreadRunner, Receiver<(JobId, RunnerEvent)>) {
    let (tx, rx) = mpsc::channel();
    let runner = ThreadRunner::new(Limits::default(), move |id, event| {
        let _ = tx.send((id, event));
    });
    (runner, rx)
}

const FRAMES: usize = 10_000;

fn s16_input(d: &Dir) {
    let s = spec(8000, SampleFormat::S16, ChannelLayout::mono());
    d.write("in.wav", &sine_wav(&s, FRAMES).1);
}

#[test]
fn converts_s16_to_s24_and_the_output_holds_the_samples() {
    let d = Dir::new("convert");
    s16_input(&d);
    let (mut r, rx) = runner();
    let id = job_id();
    let req = request(
        &d.path("in.wav"),
        &d.path("out.wav"),
        Some(SampleFormat::S24),
        false,
    );
    r.start(id, &req, CancelToken::new());
    let events = collect(&rx, id);
    assert_eq!(events.first(), Some(&RunnerEvent::Started));
    assert_eq!(
        events.last(),
        Some(&RunnerEvent::Finished {
            frames: FRAMES as u64
        })
    );
    let out = std::fs::read(d.path("out.wav")).unwrap();
    // The oracle: widen the s16 samples the input actually holds.
    let input = std::fs::read(d.path("in.wav")).unwrap();
    let held = pcm::decode_le(data_chunk(&input), SampleFormat::S16).unwrap();
    assert_eq!(held.len(), FRAMES);
    assert_eq!(data_chunk(&out), pcm::encode_le(&held, SampleFormat::S24));
    assert_eq!(d.listing(), ["in.wav", "out.wav"]);
}

#[test]
fn refuses_to_overwrite_unless_asked() {
    let d = Dir::new("overwrite");
    s16_input(&d);
    d.write("out.wav", b"precious");
    let (mut r, rx) = runner();
    let id = job_id();
    let out = d.path("out.wav");
    r.start(
        id,
        &request(&d.path("in.wav"), &out, None, false),
        CancelToken::new(),
    );
    assert_eq!(
        collect(&rx, id).last(),
        Some(&RunnerEvent::Failed {
            message: format!("{out} already exists")
        })
    );
    assert_eq!(std::fs::read(&out).unwrap(), b"precious");

    r.start(
        id,
        &request(&d.path("in.wav"), &out, None, true),
        CancelToken::new(),
    );
    assert_eq!(
        collect(&rx, id).last(),
        Some(&RunnerEvent::Finished {
            frames: FRAMES as u64
        })
    );
    assert_ne!(std::fs::read(&out).unwrap(), b"precious");
}

#[test]
fn cancelled_before_start_leaves_nothing_behind() {
    let d = Dir::new("cancel-early");
    s16_input(&d);
    let (mut r, rx) = runner();
    let id = job_id();
    let token = CancelToken::new();
    token.cancel();
    r.start(
        id,
        &request(&d.path("in.wav"), &d.path("out.wav"), None, false),
        token,
    );
    assert_eq!(collect(&rx, id).last(), Some(&RunnerEvent::Cancelled));
    assert_eq!(d.listing(), ["in.wav"]);
}

#[test]
fn cancelling_from_inside_notify_stops_the_job() {
    let d = Dir::new("cancel-mid");
    s16_input(&d);
    let (tx, rx) = mpsc::channel();
    let token = CancelToken::new();
    let inner = token.clone();
    let mut r = ThreadRunner::new(Limits::default(), move |id, event| {
        if matches!(event, RunnerEvent::Progress { .. }) {
            inner.cancel();
        }
        let _ = tx.send((id, event));
    });
    let id = job_id();
    r.start(
        id,
        &request(&d.path("in.wav"), &d.path("out.wav"), None, false),
        token,
    );
    let events = collect(&rx, id);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, RunnerEvent::Progress { .. }))
    );
    assert_eq!(events.last(), Some(&RunnerEvent::Cancelled));
    assert_eq!(d.listing(), ["in.wav"]);
}

#[test]
fn a_missing_input_fails_with_its_name() {
    let d = Dir::new("missing");
    let (mut r, rx) = runner();
    let id = job_id();
    r.start(
        id,
        &request(&d.path("nope.wav"), &d.path("out.wav"), None, false),
        CancelToken::new(),
    );
    let Some(RunnerEvent::Failed { message }) = collect(&rx, id).pop() else {
        panic!("expected Failed");
    };
    assert!(message.contains("nope.wav"), "{message}");
    assert_eq!(d.listing(), Vec::<String>::new());
}

#[test]
fn progress_is_monotonic_and_throttled() {
    const LONG: usize = 1_500_000;
    let d = Dir::new("throttle");
    let s = spec(8000, SampleFormat::U8, ChannelLayout::mono());
    d.write("in.wav", &wav_bytes(&s, &vec![128u8; LONG]));
    let (mut r, rx) = runner();
    let id = job_id();
    r.start(
        id,
        &request(
            &d.path("in.wav"),
            &d.path("out.wav"),
            Some(SampleFormat::S16),
            false,
        ),
        CancelToken::new(),
    );
    let events = collect(&rx, id);
    let progress: Vec<u64> = events
        .iter()
        .filter_map(|e| match e {
            RunnerEvent::Progress {
                frames_done,
                frames_total,
            } => {
                assert_eq!(*frames_total, Some(LONG as u64));
                Some(*frames_done)
            }
            _ => None,
        })
        .collect();
    assert!(progress.len() <= 201, "{} progress events", progress.len());
    assert!(progress.len() > 10);
    assert!(progress.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(progress.last(), Some(&(LONG as u64)));
    assert_eq!(
        events.last(),
        Some(&RunnerEvent::Finished {
            frames: LONG as u64
        })
    );
}

#[test]
fn a_worker_that_panics_still_reports_how_the_job_ended() {
    let d = Dir::new("panic");
    s16_input(&d);
    let (tx, rx) = mpsc::channel();
    // The listener blows up on the first report, taking the worker with it.
    let mut runner = ThreadRunner::new(Limits::default(), move |id, event| {
        assert!(event != RunnerEvent::Started, "listener failure");
        let _ = tx.send((id, event));
    });
    let id = job_id();
    runner.start(
        id,
        &request(&d.path("in.wav"), &d.path("out.wav"), None, false),
        CancelToken::new(),
    );
    let events = collect(&rx, id);
    assert!(
        matches!(events.as_slice(), [RunnerEvent::Failed { .. }]),
        "{events:?}"
    );
}
