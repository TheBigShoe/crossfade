//! Preview playback: a file decoded on a worker thread, converted to the output device's format
//! and handed to the device's own thread through wait-free ring buffers.
//!
//! Three threads touch a player. The UI thread owns the [`Player`] and its [`AudioSink`]. A
//! worker decodes through a `DecodeSession`, converts, and writes samples ahead into a bounded
//! ring. The sink's thread (a device callback, or a test pulling by hand) calls [`Feed::fill`],
//! which touches only atomics and rings: it never blocks, locks or allocates.
//!
//! Every load and seek starts a new *segment*: a fresh ring whose first sample is the session's
//! frame at the target. A seek therefore flushes by replacing, not by draining from the wrong
//! end of a single-consumer ring. The worker hands each segment's reading end to the feed
//! through a small ring of its own, and the feed hands the old one back, so nothing is freed on
//! the audio thread.
//!
//! The position is what the device has played. The feed counts the output frames it took from
//! the segment and maps them back to source frames through the rate ratio (`start + out *
//! in_rate / out_rate`, exact because the resampler is time-aligned), then publishes that in
//! one atomic word tagged with the segment's generation, so a stale segment's count is never
//! read as the new one's.
//!
//! Scrubbing is silent: while the pointer drags, output holds and the position follows the
//! pointer; on release the player seeks there and carries on as before. Snippets at the pointer
//! would restart decoding on every mouse event for little gain in a preview.

mod convert;
mod feed;
mod follow;
mod sink;
mod worker;

pub use convert::device_layout;
pub use feed::Feed;
pub use follow::Follow;
pub use sink::{VirtualOutput, VirtualSink};

use crossfade_core::{JobIo, Limits, StreamInfo};
use feed::{Control, unpack};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use worker::{Command, Worker};

/// What the output device runs at. Samples are always interleaved `f32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputFormat {
    pub sample_rate: u32,
    pub channels: u16,
}

/// The output device seam. The sink says what format it runs at; the player gives it a
/// [`Feed`] once, and the sink calls [`Feed::fill`] from its own thread whenever it wants
/// samples, for as long as it is started.
pub trait AudioSink {
    fn format(&self) -> OutputFormat;
    /// Takes the feed to pull from. Called once, before the first `start`.
    fn connect(&mut self, feed: Feed) -> Result<(), String>;
    /// Starts (or resumes) pulling.
    fn start(&mut self) -> Result<(), String>;
    /// Stops pulling for now; `start` resumes.
    fn pause(&mut self) -> Result<(), String>;
    /// Stops for good and drops the feed.
    fn stop(&mut self);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Nothing loaded.
    Empty,
    Loading,
    /// Loaded, not playing, never started since the load (or stopped).
    Ready,
    Playing,
    Paused,
    /// Played to the end; the position is the duration.
    Ended,
    /// The file could not be opened for playback.
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Stopped,
    Playing,
    Paused,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Load {
    Ready,
    Failed(String),
}

/// What the worker reports to the UI thread.
#[derive(Debug, Default)]
struct Status {
    /// The generation of the last load the worker finished, and how it went.
    load: Option<(u64, Load)>,
    stream: Option<StreamInfo>,
    /// Frames on the timeline; for a stream that does not say, known once decoding ends.
    duration: Option<u64>,
    /// Commands done and wake-ups seen when the worker last ran out of work.
    handled: u64,
    seen: u64,
}

pub(crate) struct Shared {
    control: Arc<Control>,
    status: Mutex<Status>,
    idle: Condvar,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A poisoned lock means the worker panicked; what it last wrote is still the best answer.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Debug, Clone, Copy)]
struct Scrub {
    frame: u64,
    was_playing: bool,
}

/// Plays one file at a time through an [`AudioSink`]. Dropping it stops the sink and the worker.
pub struct Player {
    sink: Box<dyn AudioSink>,
    shared: Arc<Shared>,
    commands: mpsc::Sender<Command>,
    worker: Option<JoinHandle<()>>,
    /// Bumped by every load and seek; the feed tags the position with it.
    generation: u64,
    /// The generation of the last load (0: never loaded).
    loaded: u64,
    /// Where the last load or seek went, the position until the feed has caught up.
    target: u64,
    sent: u64,
    mode: Mode,
    scrub: Option<Scrub>,
}

impl Player {
    /// A player over `sink`, opening files through `io`. `notify` runs on the worker thread
    /// when a load finishes or playback reaches the end; forward it to the UI thread and read
    /// the state there. Fails when the sink will not take the feed.
    pub fn new(
        sink: Box<dyn AudioSink>,
        io: Box<dyn JobIo + Send>,
        limits: Limits,
        notify: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let format = sink.format();
        if format.sample_rate == 0 || format.channels == 0 {
            return Err(format!(
                "the output reports {} Hz and {} channels",
                format.sample_rate, format.channels
            ));
        }
        let control = Arc::new(Control::default());
        let shared = Arc::new(Shared {
            control: Arc::clone(&control),
            status: Mutex::new(Status::default()),
            idle: Condvar::new(),
        });
        let (feed, ends) = Feed::new(format, Arc::clone(&control));
        let (commands, receiver) = mpsc::channel();
        let worker_shared = Arc::clone(&shared);
        let notify: Box<dyn Fn() + Send + Sync> = Box::new(notify);
        // Built on its own thread: a decode session is not `Send`.
        let handle = std::thread::Builder::new()
            .name("crossfade-player".into())
            .spawn(move || {
                Worker::new(receiver, worker_shared, io, limits, format, ends, notify).run();
            })
            .map_err(|e| format!("could not start the player: {e}"))?;
        // Before the sink can call the feed, so no wake-up is lost.
        let _ = control.worker.set(handle.thread().clone());
        let mut player = Self {
            sink,
            shared,
            commands,
            worker: Some(handle),
            generation: 0,
            loaded: 0,
            target: 0,
            sent: 0,
            mode: Mode::Stopped,
            scrub: None,
        };
        player.sink.connect(feed)?;
        Ok(player)
    }

    pub fn format(&self) -> OutputFormat {
        self.sink.format()
    }

    /// A new load or seek: from here on the feed plays only that segment.
    fn next_generation(&mut self) -> u64 {
        self.generation += 1;
        self.shared
            .control
            .current
            .store(self.generation, Ordering::Release);
        self.generation
    }

    fn send(&mut self, command: Command) {
        self.sent += 1;
        // The worker only goes away with the player.
        let _ = self.commands.send(command);
        if let Some(t) = self.shared.control.worker.get() {
            t.unpark();
        }
    }

    fn set_playing(&mut self, playing: bool) {
        self.shared
            .control
            .playing
            .store(playing, Ordering::Release);
        let result = if playing {
            self.sink.start()
        } else {
            self.sink.pause()
        };
        if let Err(e) = result {
            tracing_free_log(&e);
        }
    }

    /// Replaces whatever is loaded with `path`, stopped at its start.
    pub fn load(&mut self, path: impl Into<String>) {
        self.scrub = None;
        self.mode = Mode::Stopped;
        self.set_playing(false);
        let generation = self.next_generation();
        self.loaded = generation;
        self.target = 0;
        self.send(Command::Load {
            path: path.into(),
            generation,
        });
    }

    fn has_audio(&self) -> bool {
        matches!(
            self.state(),
            State::Ready | State::Playing | State::Paused | State::Ended | State::Loading
        )
    }

    pub fn play(&mut self) {
        match self.state() {
            State::Ended => {
                self.seek(0);
            }
            State::Empty | State::Failed(_) | State::Playing => return,
            _ => {}
        }
        if let Some(scrub) = &mut self.scrub {
            scrub.was_playing = true;
            return;
        }
        self.mode = Mode::Playing;
        self.set_playing(true);
    }

    pub fn pause(&mut self) {
        if let Some(scrub) = &mut self.scrub {
            scrub.was_playing = false;
            return;
        }
        if self.mode == Mode::Playing {
            self.mode = Mode::Paused;
            self.set_playing(false);
        }
    }

    pub fn toggle(&mut self) {
        let playing = match self.scrub {
            Some(s) => s.was_playing,
            None => self.state() == State::Playing,
        };
        if playing { self.pause() } else { self.play() }
    }

    /// Stops and goes back to the start.
    pub fn stop(&mut self) {
        if !self.has_audio() {
            return;
        }
        self.scrub = None;
        self.set_playing(false);
        self.seek(0);
        self.mode = Mode::Stopped;
    }

    /// Jumps to `frame` of the source (held to the duration when it is known): the next frame
    /// played is the session's frame there. Playing carries on from it; paused stays paused.
    pub fn seek(&mut self, frame: u64) {
        if !self.has_audio() {
            return;
        }
        if self.state() == State::Ended {
            self.mode = Mode::Paused;
            self.set_playing(false);
        }
        let frame = self.duration().map_or(frame, |d| frame.min(d));
        let generation = self.next_generation();
        self.target = frame;
        self.send(Command::Seek { frame, generation });
    }

    /// Follows a drag: output holds silent and the position is `frame` until
    /// [`end_scrub`](Self::end_scrub).
    pub fn scrub_to(&mut self, frame: u64) {
        if !self.has_audio() {
            return;
        }
        let frame = self.duration().map_or(frame, |d| frame.min(d));
        match &mut self.scrub {
            Some(s) => s.frame = frame,
            None => {
                let was_playing = self.state() == State::Playing;
                if was_playing {
                    self.set_playing(false);
                }
                self.scrub = Some(Scrub { frame, was_playing });
            }
        }
    }

    /// The drag ended: seek to where it was let go and carry on as before it began.
    pub fn end_scrub(&mut self) {
        let Some(scrub) = self.scrub.take() else {
            return;
        };
        self.seek(scrub.frame);
        if scrub.was_playing {
            self.mode = Mode::Playing;
            self.set_playing(true);
        }
    }

    pub fn is_scrubbing(&self) -> bool {
        self.scrub.is_some()
    }

    pub fn state(&self) -> State {
        if self.loaded == 0 {
            return State::Empty;
        }
        let load = lock(&self.shared.status)
            .load
            .clone()
            .filter(|(g, _)| *g == self.loaded);
        match load {
            None => State::Loading,
            Some((_, Load::Failed(why))) => State::Failed(why),
            Some((_, Load::Ready)) => match self.mode {
                Mode::Stopped => State::Ready,
                Mode::Paused => State::Paused,
                Mode::Playing if self.ended() => State::Ended,
                Mode::Playing => State::Playing,
            },
        }
    }

    fn ended(&self) -> bool {
        self.shared.control.ended.load(Ordering::Acquire) == self.generation + 1
    }

    /// Frames of the source played out so far (or where a seek or drag put it).
    pub fn position(&self) -> u64 {
        if let Some(s) = self.scrub {
            return s.frame;
        }
        let (generation, frames) = unpack(self.shared.control.position.load(Ordering::Acquire));
        let at = if generation == feed::tag(self.generation) {
            frames
        } else {
            self.target
        };
        self.duration().map_or(at, |d| at.min(d))
    }

    /// Frames on the timeline; `None` until known (a stream that does not say has one once
    /// decoding reaches its end).
    pub fn duration(&self) -> Option<u64> {
        let status = lock(&self.shared.status);
        status
            .load
            .as_ref()
            .filter(|(g, l)| *g == self.loaded && *l == Load::Ready)
            .and(status.duration)
    }

    /// The loaded stream, once open.
    pub fn stream(&self) -> Option<StreamInfo> {
        let status = lock(&self.shared.status);
        status
            .load
            .as_ref()
            .filter(|(g, l)| *g == self.loaded && *l == Load::Ready)
            .and(status.stream.clone())
    }

    /// Times the output wanted samples the worker had not yet decoded (silence went out).
    pub fn underruns(&self) -> u64 {
        self.shared.control.underruns.load(Ordering::Relaxed)
    }

    /// Output frames the sink has asked for, silence included.
    pub fn pulled(&self) -> u64 {
        self.shared.control.pulled.load(Ordering::Relaxed)
    }

    /// Blocks until the worker has carried out every command and filled the buffer as far as
    /// it can (full, or the end of the file). Tests and screenshots, which must not race it.
    pub fn settle(&self) {
        let control = &self.shared.control;
        let mut status = lock(&self.shared.status);
        while status.handled != self.sent || status.seen != control.requests.load(Ordering::SeqCst)
        {
            status = self
                .shared
                .idle
                .wait(status)
                .unwrap_or_else(|e| e.into_inner());
        }
    }
}

/// The app crate has no logger; a sink error is not worth failing an action over.
fn tracing_free_log(error: &str) {
    eprintln!("crossfade: audio output: {error}");
}

impl Drop for Player {
    fn drop(&mut self) {
        self.sink.stop();
        let _ = self.commands.send(Command::Quit);
        if let Some(handle) = self.worker.take() {
            handle.thread().unpark();
            let _ = handle.join();
        }
    }
}

/// `1:23.4`: minutes, seconds and tenths, hours when there are any.
pub fn clock_text(frames: u64, rate: u32) -> String {
    if rate == 0 {
        return "0:00.0".into();
    }
    let tenths = u128::from(frames) * 10 / u128::from(rate);
    let (whole, tenth) = (tenths / 10, tenths % 10);
    let (h, m, s) = (whole / 3600, whole / 60 % 60, whole % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}.{tenth}")
    } else {
        format!("{m}:{s:02}.{tenth}")
    }
}

/// The transport's readout, `1:23.4 / 4:56.7`; just the position when the length is unknown.
pub fn transport_text(position: u64, duration: Option<u64>, rate: u32) -> String {
    match duration {
        Some(d) => format!("{} / {}", clock_text(position, rate), clock_text(d, rate)),
        None => clock_text(position, rate),
    }
}
