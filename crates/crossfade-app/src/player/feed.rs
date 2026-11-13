//! The audio thread's side: what [`Feed::fill`] reads. Atomics and rings only.

use super::OutputFormat;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::Thread;

/// Segments in flight from the worker to the feed, at most.
pub(crate) const PENDING_SEGMENTS: usize = 2;

const FRAME_BITS: u32 = 48;
const FRAME_MASK: u64 = (1 << FRAME_BITS) - 1;

/// The low bits of a generation, as the position word carries them.
pub(crate) fn tag(generation: u64) -> u64 {
    generation & ((1 << (64 - FRAME_BITS)) - 1)
}

/// A generation tag and a frame count in one word, so they are read together. 48 bits of
/// frames is 200 years at 44.1 kHz.
pub(crate) fn pack(generation: u64, frames: u64) -> u64 {
    (tag(generation) << FRAME_BITS) | (frames & FRAME_MASK)
}

pub(crate) fn unpack(word: u64) -> (u64, u64) {
    (word >> FRAME_BITS, word & FRAME_MASK)
}

/// Shared by the player, the worker and the feed.
#[derive(Debug, Default)]
pub(crate) struct Control {
    pub(crate) playing: AtomicBool,
    /// The generation the feed may play; older segments are silenced.
    pub(crate) current: AtomicU64,
    /// [`pack`]ed generation and source frame played up to.
    pub(crate) position: AtomicU64,
    /// One more than the generation whose segment played to its end (0: none).
    pub(crate) ended: AtomicU64,
    pub(crate) underruns: AtomicU64,
    pub(crate) pulled: AtomicU64,
    /// A wake-up of the worker is pending; set by the feed, cleared by the worker.
    pub(crate) wake: AtomicBool,
    /// Wake-ups the feed has asked for.
    pub(crate) requests: AtomicU64,
    pub(crate) worker: OnceLock<Thread>,
}

impl Control {
    pub(crate) fn wake_worker(&self) {
        if !self.wake.swap(true, Ordering::SeqCst) {
            self.requests.fetch_add(1, Ordering::SeqCst);
            // `unpark` neither blocks nor allocates.
            if let Some(t) = self.worker.get() {
                t.unpark();
            }
        }
    }
}

/// What the worker says about a segment as it writes.
#[derive(Debug, Default)]
pub(crate) struct SegmentState {
    /// Every sample is in the ring; `end` is set.
    pub(crate) eof: AtomicBool,
    /// The source frame the segment ends at.
    pub(crate) end: AtomicU64,
}

/// The reading end of one segment.
pub(crate) struct Segment {
    pub(crate) generation: u64,
    /// The source frame of its first sample.
    pub(crate) start: u64,
    pub(crate) source_rate: u32,
    pub(crate) samples: rtrb::Consumer<f32>,
    pub(crate) state: Arc<SegmentState>,
    /// Output frames taken from it.
    played: u64,
    ended: bool,
}

impl Segment {
    pub(crate) fn new(
        generation: u64,
        start: u64,
        source_rate: u32,
        samples: rtrb::Consumer<f32>,
        state: Arc<SegmentState>,
    ) -> Self {
        Self {
            generation,
            start,
            source_rate,
            samples,
            state,
            played: 0,
            ended: false,
        }
    }
}

/// The worker's ends of the two segment rings.
pub(crate) struct Ends {
    pub(crate) outgoing: rtrb::Producer<Segment>,
    pub(crate) retired: rtrb::Consumer<Segment>,
}

/// What a sink pulls samples from, on its own thread. [`fill`](Self::fill) never blocks, locks
/// or allocates; when the worker has nothing ready it writes silence.
pub struct Feed {
    channels: usize,
    rate: u64,
    control: Arc<Control>,
    incoming: rtrb::Consumer<Segment>,
    retired: rtrb::Producer<Segment>,
    current: Option<Segment>,
}

impl Feed {
    pub(crate) fn new(format: OutputFormat, control: Arc<Control>) -> (Self, Ends) {
        let (outgoing, incoming) = rtrb::RingBuffer::new(PENDING_SEGMENTS);
        // Room for every segment the feed can hold or adopt at once.
        let (retired, retired_rx) = rtrb::RingBuffer::new(PENDING_SEGMENTS + 1);
        (
            Self {
                channels: usize::from(format.channels),
                rate: u64::from(format.sample_rate),
                control,
                incoming,
                retired,
                current: None,
            },
            Ends {
                outgoing,
                retired: retired_rx,
            },
        )
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Fills `out` (interleaved, a whole number of frames) with the next samples, or silence.
    pub fn fill(&mut self, out: &mut [f32]) {
        self.adopt();
        let ch = self.channels;
        let frames = out.len() / ch;
        self.control
            .pulled
            .fetch_add(frames as u64, Ordering::Relaxed);
        let playing = self.control.playing.load(Ordering::Acquire);
        let current = self.control.current.load(Ordering::Acquire);
        let Some(seg) = self
            .current
            .as_mut()
            .filter(|s| playing && !s.ended && s.generation == current)
        else {
            out.fill(0.0);
            return;
        };
        // Read before the ring: a segment marked complete has all its samples visible.
        let eof = seg.state.eof.load(Ordering::Acquire);
        let n = (frames * ch).min(seg.samples.slots() / ch * ch);
        if seg.samples.pop_entire_slice(&mut out[..n]).is_err() {
            out.fill(0.0);
            return;
        }
        out[n..].fill(0.0);
        let got = (n / ch) as u64;
        let heard = seg.played > 0 || got > 0;
        seg.played += got;
        let rate = self.rate;
        let at = seg.start.saturating_add(
            u64::try_from(u128::from(seg.played) * u128::from(seg.source_rate) / u128::from(rate))
                .unwrap_or(u64::MAX),
        );
        if (got as usize) < frames {
            if eof {
                seg.ended = true;
                let end = seg.state.end.load(Ordering::Acquire);
                self.control
                    .position
                    .store(pack(seg.generation, end), Ordering::Release);
                self.control
                    .ended
                    .store(seg.generation + 1, Ordering::Release);
                self.control.wake_worker();
                return;
            }
            // Waiting for the first samples after a load or seek is not a dropout.
            if heard {
                self.control.underruns.fetch_add(1, Ordering::Relaxed);
            }
        }
        if got > 0 {
            self.control
                .position
                .store(pack(seg.generation, at), Ordering::Release);
        }
        self.control.wake_worker();
    }

    /// Takes the newest segment the worker sent, handing the one it replaces back.
    fn adopt(&mut self) {
        while self.retired.slots() > 0 {
            let Ok(next) = self.incoming.pop() else {
                return;
            };
            self.control
                .position
                .store(pack(next.generation, next.start), Ordering::Release);
            if let Some(old) = self.current.replace(next) {
                let _ = self.retired.push(old);
            }
        }
    }
}
