//! A sink with no device behind it, for tests and screenshots: whoever holds its
//! [`VirtualOutput`] pulls samples when they choose, by count or by a [`Clock`].

use super::{AudioSink, Feed, OutputFormat};
use crossfade_core::Clock;
use std::sync::{Arc, Mutex};

/// Frames per pull when following a clock, about what a device asks for at a time.
const BLOCK: usize = 512;

#[derive(Default)]
struct Inner {
    feed: Option<Feed>,
    running: bool,
    /// Clock-driven pulls: the device frames due so far, pulled or not.
    due: u64,
}

pub struct VirtualSink {
    format: OutputFormat,
    inner: Arc<Mutex<Inner>>,
}

/// The test's side of a [`VirtualSink`].
#[derive(Clone)]
pub struct VirtualOutput {
    format: OutputFormat,
    inner: Arc<Mutex<Inner>>,
}

impl VirtualSink {
    pub fn new(format: OutputFormat) -> (Self, VirtualOutput) {
        let inner = Arc::new(Mutex::new(Inner::default()));
        (
            Self {
                format,
                inner: Arc::clone(&inner),
            },
            VirtualOutput { format, inner },
        )
    }
}

fn lock(inner: &Mutex<Inner>) -> std::sync::MutexGuard<'_, Inner> {
    inner.lock().unwrap_or_else(|e| e.into_inner())
}

impl AudioSink for VirtualSink {
    fn format(&self) -> OutputFormat {
        self.format
    }

    fn connect(&mut self, feed: Feed) -> Result<(), String> {
        lock(&self.inner).feed = Some(feed);
        Ok(())
    }

    fn start(&mut self) -> Result<(), String> {
        lock(&self.inner).running = true;
        Ok(())
    }

    fn pause(&mut self) -> Result<(), String> {
        lock(&self.inner).running = false;
        Ok(())
    }

    fn stop(&mut self) {
        let mut inner = lock(&self.inner);
        inner.running = false;
        inner.feed = None;
    }
}

impl VirtualOutput {
    pub fn format(&self) -> OutputFormat {
        self.format
    }

    /// Whether the player has the sink started, as a device would be calling back.
    pub fn is_running(&self) -> bool {
        lock(&self.inner).running
    }

    /// Pulls `frames` frames, interleaved, as a device callback would. A sink that is not
    /// started (or has been stopped) pulls nothing: the result is empty.
    pub fn pull(&self, frames: usize) -> Vec<f32> {
        let mut inner = lock(&self.inner);
        if !inner.running {
            return Vec::new();
        }
        let Some(feed) = inner.feed.as_mut() else {
            return Vec::new();
        };
        let mut out = vec![0.0; frames * usize::from(self.format.channels)];
        feed.fill(&mut out);
        out
    }

    /// Pulls, in device-sized blocks, what a device at this rate would have by `clock`'s time
    /// (measured from zero). Time while the sink is stopped passes without pulling.
    pub fn run_until(&self, clock: &dyn Clock) -> Vec<f32> {
        let now = clock.now();
        let total = u64::try_from(
            now.as_nanos() * u128::from(self.format.sample_rate) / 1_000_000_000,
        )
        .unwrap_or(u64::MAX);
        let mut out = Vec::new();
        loop {
            let (step, running) = {
                let mut inner = lock(&self.inner);
                let step = total.saturating_sub(inner.due).min(BLOCK as u64);
                inner.due += step;
                (step, inner.running)
            };
            if step == 0 {
                return out;
            }
            if running {
                out.extend(self.pull(step as usize));
            }
        }
    }
}
