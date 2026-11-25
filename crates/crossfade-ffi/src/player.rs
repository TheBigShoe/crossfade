//! Preview playback across the boundary. The native app owns the audio device and pulls: the
//! app's `Player` runs over a sink with no device behind it, whose `Feed` the app takes once
//! and calls from its own audio callback.
//!
//! A `Player` holds its sink as a `Box<dyn AudioSink>`, which is not `Send` (a device stream
//! often is not), so a player is driven only from the thread that made it; that is checked
//! on every call. Waiting touches only the revision counter, so any thread may wait.

use crate::error::{CF_ERR_WRONG_THREAD, Failure};
use crate::signal::Signal;
use crossfade_app::FsIo;
use crossfade_app::player::{AudioSink, Feed, OutputFormat, Player, State, transport_text};
use crossfade_core::Limits;
use serde::Serialize;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::Duration;

/// A sink the native app pulls from: it keeps the feed for the app to take and remembers
/// whether the player wants the device running.
struct PulledSink {
    format: OutputFormat,
    slot: Arc<Mutex<Option<Feed>>>,
    started: Arc<AtomicBool>,
}

impl AudioSink for PulledSink {
    fn format(&self) -> OutputFormat {
        self.format
    }

    fn connect(&mut self, feed: Feed) -> Result<(), String> {
        *self.slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(feed);
        Ok(())
    }

    fn start(&mut self) -> Result<(), String> {
        self.started.store(true, Ordering::Release);
        Ok(())
    }

    fn pause(&mut self) -> Result<(), String> {
        self.started.store(false, Ordering::Release);
        Ok(())
    }

    fn stop(&mut self) {
        self.started.store(false, Ordering::Release);
    }
}

pub struct NativePlayer {
    owner: ThreadId,
    /// The one part any thread may touch: the ABI's wait borrows this field alone.
    pub(crate) signal: Arc<Signal>,
    started: Arc<AtomicBool>,
    feed: RefCell<Option<Feed>>,
    player: RefCell<Player>,
}

#[derive(Serialize)]
struct Output {
    sample_rate: u32,
    channels: u16,
}

#[derive(Serialize)]
struct StateJson {
    revision: u64,
    /// `empty`, `loading`, `ready`, `playing`, `paused`, `ended` or `failed`.
    state: &'static str,
    failure: Option<String>,
    position: u64,
    duration: Option<u64>,
    sample_rate: Option<u32>,
    channels: Option<u16>,
    transport: Option<String>,
    scrubbing: bool,
    device_running: bool,
    underruns: u64,
    pulled: u64,
    output: Output,
}

fn wrong_thread() -> Failure {
    Failure::new(
        CF_ERR_WRONG_THREAD,
        "A player is used only from the thread that made it.",
    )
}

impl NativePlayer {
    /// A player for a device at `sample_rate` with `channels` channels.
    pub fn new(sample_rate: u32, channels: u16) -> Result<Self, Failure> {
        let signal = Arc::new(Signal::default());
        let slot = Arc::new(Mutex::new(None));
        let started = Arc::new(AtomicBool::new(false));
        let sink = PulledSink {
            format: OutputFormat {
                sample_rate,
                channels,
            },
            slot: Arc::clone(&slot),
            started: Arc::clone(&started),
        };
        let notify = Arc::clone(&signal);
        let player = Player::new(
            Box::new(sink),
            Box::new(FsIo::new()),
            Limits::default(),
            move || notify.bump(),
        )
        .map_err(|why| Failure::refused(format!("The player cannot start: {why}.")))?;
        let feed = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
        Ok(Self {
            owner: std::thread::current().id(),
            signal,
            started,
            feed: RefCell::new(feed),
            player: RefCell::new(player),
        })
    }

    /// Whether the calling thread may drive this player.
    pub fn check_thread(&self) -> Result<(), Failure> {
        if std::thread::current().id() == self.owner {
            Ok(())
        } else {
            Err(wrong_thread())
        }
    }

    /// Runs `f` on the player from its own thread and says it changed.
    fn drive<R>(&self, f: impl FnOnce(&mut Player) -> R) -> Result<R, Failure> {
        self.check_thread()?;
        let out = f(&mut self.player.borrow_mut());
        self.signal.bump();
        Ok(out)
    }

    /// The pull side, once.
    pub fn take_feed(&self) -> Result<PullFeed, Failure> {
        self.check_thread()?;
        let feed = self
            .feed
            .borrow_mut()
            .take()
            .ok_or_else(|| Failure::refused("The player's feed has already been taken."))?;
        Ok(PullFeed::new(feed))
    }

    pub fn load(&self, path: &str) -> Result<(), Failure> {
        self.drive(|p| p.load(path))
    }

    pub fn play(&self) -> Result<(), Failure> {
        self.drive(Player::play)
    }

    pub fn pause(&self) -> Result<(), Failure> {
        self.drive(Player::pause)
    }

    pub fn toggle(&self) -> Result<(), Failure> {
        self.drive(Player::toggle)
    }

    pub fn stop(&self) -> Result<(), Failure> {
        self.drive(Player::stop)
    }

    pub fn seek(&self, frame: u64) -> Result<(), Failure> {
        self.drive(|p| p.seek(frame))
    }

    pub fn scrub(&self, frame: u64) -> Result<(), Failure> {
        self.drive(|p| p.scrub_to(frame))
    }

    pub fn end_scrub(&self) -> Result<(), Failure> {
        self.drive(Player::end_scrub)
    }

    /// Blocks until the decoder has carried out every command and filled what it can.
    pub fn settle(&self) -> Result<(), Failure> {
        self.check_thread()?;
        self.player.borrow().settle();
        Ok(())
    }

    pub fn position(&self) -> Result<u64, Failure> {
        self.check_thread()?;
        Ok(self.player.borrow().position())
    }

    pub(crate) fn wait(signal: &Signal, since: u64, timeout: Duration) -> u64 {
        signal.wait(since, timeout)
    }

    pub fn state_json(&self) -> Result<String, Failure> {
        self.check_thread()?;
        let revision = self.signal.revision();
        let p = self.player.borrow();
        let (state, failure) = match p.state() {
            State::Empty => ("empty", None),
            State::Loading => ("loading", None),
            State::Ready => ("ready", None),
            State::Playing => ("playing", None),
            State::Paused => ("paused", None),
            State::Ended => ("ended", None),
            State::Failed(why) => ("failed", Some(why)),
        };
        let stream = p.stream();
        let rate = stream.as_ref().map(|s| s.audio.sample_rate());
        let position = p.position();
        let duration = p.duration();
        let format = p.format();
        let json = StateJson {
            revision,
            state,
            failure,
            position,
            duration,
            sample_rate: rate,
            channels: stream.as_ref().map(|s| s.audio.layout().channels()),
            transport: rate.map(|r| transport_text(position, duration, r)),
            scrubbing: p.is_scrubbing(),
            device_running: self.started.load(Ordering::Acquire),
            underruns: p.underruns(),
            pulled: p.pulled(),
            output: Output {
                sample_rate: format.sample_rate,
                channels: format.channels,
            },
        };
        Ok(serde_json::to_string(&json).unwrap_or_default())
    }
}

impl Drop for NativePlayer {
    fn drop(&mut self) {
        // A feed that outlives the player plays silence, not what was left in its ring.
        self.player.get_mut().pause();
    }
}

/// Frames deinterleaved per pass of a planar fill; the scratch is allocated once, up front.
const PLANAR_FRAMES: usize = 1024;

/// The audio callback's side: the player's `Feed` plus a scratch buffer for planar output.
/// Its fills touch only atomics, the feed's rings and memory allocated here.
pub struct PullFeed {
    feed: Feed,
    scratch: Box<[f32]>,
}

impl PullFeed {
    fn new(feed: Feed) -> Self {
        let scratch = vec![0.0; PLANAR_FRAMES * feed.channels()].into_boxed_slice();
        Self { feed, scratch }
    }

    pub fn channels(&self) -> usize {
        self.feed.channels()
    }

    /// Interleaved samples, or silence.
    pub fn fill(&mut self, out: &mut [f32]) {
        self.feed.fill(out);
    }

    /// `frames` frames in passes through the scratch; `put(first_frame, interleaved)` writes
    /// each pass out to the planes.
    pub fn fill_planar(&mut self, frames: usize, mut put: impl FnMut(usize, &[f32])) {
        let channels = self.feed.channels();
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(PLANAR_FRAMES);
            let chunk = &mut self.scratch[..n * channels];
            self.feed.fill(chunk);
            put(done, chunk);
            done += n;
        }
    }
}
