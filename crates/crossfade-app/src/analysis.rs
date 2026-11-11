//! One background pass per file that yields both the waveform and the loudness readout.
//!
//! The worker decodes with `crossfade_graph::decode_frames` and feeds the [`Pyramid`] and a
//! `crossfade_loudness::Meter` from the same frames. The UI polls [`Analyzer::snapshot`]
//! (a few words, no pyramid copy), draws from the live data with [`Analyzer::with`] (a brief
//! lock; the worker holds it only to append one decoded frame's blocks), or is told by the
//! `notify` callback, which runs on the worker thread and is throttled by frame count.

use crate::waveform::Pyramid;
use crossfade_core::{
    AudioFrame, Channel, ChannelLayout, Error, JobIo, Limits, Samples, StreamInfo,
};
use crossfade_graph::{CancelToken, decode_frames};
use crossfade_loudness::{ChannelRole, Meter, Options};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

/// Frames between progress reports when the file's length is unknown.
const UNKNOWN_TOTAL_STEP: u64 = 65_536;

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Running,
    Done,
    Failed(String),
    Cancelled,
}

/// Linear peaks (`crossfade_loudness::to_db` converts), loudness in LUFS / LU. `None` means
/// no loudness yet or at all: under 400 ms (3 s for range), silence, or a sample rate the
/// meter refuses (the waveform still works). While running these are refreshed with each
/// progress report; they are final when the state is `Done`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Loudness {
    pub integrated: Option<f64>,
    pub range: Option<f64>,
    pub true_peak: Option<f64>,
    pub sample_peak: f64,
}

/// Everything a view draws.
#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    /// `None` until the first frame (or the end of an empty file).
    pub stream: Option<StreamInfo>,
    pub frames_done: u64,
    pub state: State,
    pub pyramid: Pyramid,
    pub loudness: Loudness,
    /// Short-term (3 s) loudness in LUFS, one entry per 100 ms of programme: entry `i` is the
    /// value after `(i + 1) * rate / 10` frames. `f32::NEG_INFINITY` where there is none
    /// (the first 3 s, silence). Never NaN.
    pub short_term: Vec<f32>,
    /// Bumped on every change, so a view can tell whether to redraw.
    pub version: u64,
}

impl Analysis {
    fn new() -> Self {
        Self {
            stream: None,
            frames_done: 0,
            state: State::Running,
            pyramid: Pyramid::new(0),
            loudness: Loudness::default(),
            short_term: Vec::new(),
            version: 0,
        }
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            version: self.version,
            stream: self.stream.clone(),
            frames_done: self.frames_done,
            state: self.state.clone(),
            loudness: self.loudness.clone(),
        }
    }
}

/// The cheap part of an [`Analysis`]: no pyramid, no curve.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub version: u64,
    pub stream: Option<StreamInfo>,
    pub frames_done: u64,
    pub state: State,
    pub loudness: Loudness,
}

/// Each channel's loudness role from its speaker position; unpositioned layouts get the
/// standard roles for 1 to 6 channels, otherwise every channel counts once.
pub fn roles_for(layout: &ChannelLayout) -> Vec<ChannelRole> {
    let n = usize::from(layout.channels());
    if n == 1 {
        return vec![ChannelRole::Unweighted];
    }
    let Some(positions) = layout.positions() else {
        return ChannelRole::standard_layout(n)
            .map_or_else(|| vec![ChannelRole::Unweighted; n], <[_]>::to_vec);
    };
    positions
        .iter()
        .map(|p| match p {
            Channel::FrontLeft => ChannelRole::Left,
            Channel::FrontRight => ChannelRole::Right,
            Channel::FrontCenter => ChannelRole::Centre,
            Channel::Lfe => ChannelRole::Lfe,
            Channel::BackLeft | Channel::SideLeft => ChannelRole::LeftSurround,
            Channel::BackRight | Channel::SideRight => ChannelRole::RightSurround,
            _ => ChannelRole::Unweighted,
        })
        .collect()
}

/// Decides which progress reports are worth sending, from frame counts alone (rule 5).
struct Throttle {
    last: u64,
}

impl Throttle {
    fn admit(&mut self, done: u64, total: Option<u64>) -> bool {
        let step = total.map_or(UNKNOWN_TOTAL_STEP, |t| (t / 200).max(1));
        let admit = done >= self.last.saturating_add(step);
        if admit {
            self.last = done;
        }
        admit
    }
}

struct Shared {
    analysis: Mutex<Analysis>,
    /// Set after the last notification, so `wait` returns when everything has been said.
    finished: Mutex<bool>,
    changed: Condvar,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A poisoned lock only means the worker panicked; the guard below reports that.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    fn new() -> Self {
        Self {
            analysis: Mutex::new(Analysis::new()),
            finished: Mutex::new(false),
            changed: Condvar::new(),
        }
    }

    fn update(&self, f: impl FnOnce(&mut Analysis)) -> Snapshot {
        let mut a = lock(&self.analysis);
        f(&mut a);
        a.version += 1;
        a.snapshot()
    }
}

/// Reports a failure if the worker unwinds before saying how the pass ended.
struct EndGuard<'a>(&'a Shared);

impl Drop for EndGuard<'_> {
    fn drop(&mut self) {
        if lock(&self.0.analysis).state == State::Running {
            self.0
                .update(|a| a.state = State::Failed("the analyser stopped unexpectedly".into()));
        }
        *lock(&self.0.finished) = true;
        self.0.changed.notify_all();
    }
}

fn run_pass(
    path: &str,
    io: &mut dyn JobIo,
    limits: &Limits,
    cancel: &CancelToken,
    shared: &Shared,
    notify: &dyn Fn(&Snapshot),
) {
    let mut meter: Option<Meter> = None;
    let mut ticks = 0u64;
    let mut throttle = Throttle { last: 0 };
    let result = decode_frames(path, io, limits, cancel, &mut |stream, frame| {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if frame.frames() == 0 {
            return Ok(());
        }
        let first = lock(&shared.analysis).stream.is_none();
        if first {
            let roles = roles_for(&frame.layout);
            // A rate the meter refuses leaves loudness empty; the waveform needs none of it.
            meter = Meter::new(stream.audio.sample_rate(), &roles, Options::default()).ok();
        }
        let curve = feed_meter(&mut meter, frame, &mut ticks);
        let done_after = lock(&shared.analysis).frames_done + frame.frames() as u64;
        let report = throttle.admit(done_after, stream.frames);
        let loudness = (report || stream.frames == Some(done_after))
            .then(|| meter.as_ref().map(measure))
            .flatten();
        let mut result = Ok(());
        let snap = shared.update(|a| {
            if first {
                a.stream = Some(stream.clone());
                a.pyramid = Pyramid::new(frame.layout.channels());
            }
            result = a.pyramid.push(frame);
            a.frames_done = done_after;
            a.short_term.extend(curve);
            if let Some(l) = loudness {
                a.loudness = l;
            }
        });
        if report {
            notify(&snap);
        }
        result
    });
    let end = match result {
        Ok(stream) => {
            let loudness = meter.as_ref().map(measure);
            Ok((stream, loudness))
        }
        Err(e) => Err(e),
    };
    let snap = shared.update(|a| match end {
        Ok((stream, loudness)) => {
            a.stream.get_or_insert(stream);
            if let Some(l) = loudness {
                a.loudness = l;
            }
            a.state = State::Done;
        }
        Err(Error::Cancelled) => a.state = State::Cancelled,
        Err(e) => a.state = State::Failed(e.to_string()),
    });
    notify(&snap);
}

fn measure(m: &Meter) -> Loudness {
    let r = m.measurements();
    Loudness {
        integrated: r.integrated,
        range: r.loudness_range,
        true_peak: r.true_peak,
        sample_peak: r.sample_peak,
    }
}

/// Pushes `frame` into the meter, splitting at every 100 ms of programme to read the
/// short-term value there (the meter's result does not depend on the split).
fn feed_meter(meter: &mut Option<Meter>, frame: &AudioFrame, ticks: &mut u64) -> Vec<f32> {
    let (Some(meter), Samples::F64(samples)) = (meter.as_mut(), &frame.samples) else {
        return Vec::new();
    };
    let channels = usize::from(frame.layout.channels());
    let rate = u64::from(frame.sample_rate);
    let mut out = Vec::new();
    let mut at = 0;
    let total = samples.len() / channels;
    while at < total {
        let next = (*ticks + 1) * rate / 10;
        let room = next.saturating_sub(meter.frames()).max(1) as usize;
        let n = room.min(total - at);
        meter.push(&samples[at * channels..(at + n) * channels]);
        at += n;
        if meter.frames() >= next {
            *ticks += 1;
            out.push(meter.short_term().map_or(f32::NEG_INFINITY, |v| v as f32));
        }
    }
    out
}

/// The whole pass on the calling thread, for tests and tools; [`Analyzer`] runs the same
/// code on a worker.
pub fn analyze(path: &str, io: &mut dyn JobIo, limits: &Limits, cancel: &CancelToken) -> Analysis {
    let shared = Shared::new();
    run_pass(path, io, limits, cancel, &shared, &|_| {});
    shared
        .analysis
        .into_inner()
        .unwrap_or_else(|e| e.into_inner())
}

/// Analyses one file on a worker thread. Dropping it cancels the pass.
pub struct Analyzer {
    shared: Arc<Shared>,
    cancel: CancelToken,
}

impl Analyzer {
    /// `notify` runs on the worker thread after every 1/200 of the file (every 65,536 frames
    /// when the length is unknown) and once more when the pass ends; forward it to the UI
    /// thread and read [`snapshot`](Self::snapshot) there.
    pub fn start(
        path: impl Into<String>,
        io: Box<dyn JobIo + Send>,
        limits: Limits,
        notify: impl Fn(&Snapshot) + Send + Sync + 'static,
    ) -> Self {
        Self::start_with_cancel(path, io, limits, CancelToken::new(), notify)
    }

    pub fn start_with_cancel(
        path: impl Into<String>,
        mut io: Box<dyn JobIo + Send>,
        limits: Limits,
        cancel: CancelToken,
        notify: impl Fn(&Snapshot) + Send + Sync + 'static,
    ) -> Self {
        let shared = Arc::new(Shared::new());
        let (path, worker_shared, worker_cancel) =
            (path.into(), Arc::clone(&shared), cancel.clone());
        std::thread::spawn(move || {
            let _guard = EndGuard(&worker_shared);
            run_pass(
                &path,
                &mut *io,
                &limits,
                &worker_cancel,
                &worker_shared,
                &notify,
            );
        });
        Self { shared, cancel }
    }

    pub fn snapshot(&self) -> Snapshot {
        lock(&self.shared.analysis).snapshot()
    }

    /// Reads the live data (the pyramid to draw, the curve). Keep `f` short: the worker waits.
    pub fn with<R>(&self, f: impl FnOnce(&Analysis) -> R) -> R {
        f(&lock(&self.shared.analysis))
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Blocks until the pass has ended and its last notification has been sent.
    pub fn wait(&self) -> Snapshot {
        let mut done = lock(&self.shared.finished);
        while !*done {
            done = self
                .shared
                .changed
                .wait(done)
                .unwrap_or_else(|e| e.into_inner());
        }
        drop(done);
        self.snapshot()
    }
}

impl Drop for Analyzer {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
