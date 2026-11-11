//! Sample-rate conversion, a thin wrapper over `crossfade-resample`: its output bit for bit,
//! time-aligned (output frame `n` is the signal at `n / rate` seconds), `ceil(N * out / in)`
//! frames for `N` in, whatever the framing. The look-ahead (`latency()` input frames) is held
//! inside the resampler and released by `drain`.
//!
//! Band-limiting rings: a full-scale square wave can come out at +3.2 dB, so integer output
//! may clip, and the chain counts it.

use crossfade_core::{AudioFrame, AudioSpec, Error, Filter, Samples};
use crossfade_resample::Resampler;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Quality {
    Fast,
    #[default]
    Good,
    Best,
}

impl Quality {
    pub const NAMES: [&'static str; 3] = ["fast", "good", "best"];

    pub fn name(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Good => "good",
            Self::Best => "best",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "fast" => Some(Self::Fast),
            "good" => Some(Self::Good),
            "best" => Some(Self::Best),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResamplePlan {
    /// Output rate, Hz.
    pub rate: u32,
    pub quality: Quality,
}

impl ResamplePlan {
    /// The resampler's promise: `ceil(N * out / in)` frames, time-aligned. `None` when the
    /// input length is unknown or the count does not fit a pts.
    pub fn output_frames(&self, input_rate: u32, frames: Option<u64>) -> Option<u64> {
        Some(crossfade_resample::output_len(
            frames?, input_rate, self.rate,
        ))
        .filter(|&f| f <= i64::MAX as u64)
    }
}

impl Quality {
    fn preset(self) -> crossfade_resample::Quality {
        match self {
            Self::Fast => crossfade_resample::Quality::Fast,
            Self::Good => crossfade_resample::Quality::Balanced,
            Self::Best => crossfade_resample::Quality::Best,
        }
    }
}

pub struct Resample {
    spec: AudioSpec,
    channels: usize,
    resampler: Resampler,
    /// Converted samples not yet handed out.
    buf: Vec<f64>,
    /// pts of the next frame out, from the first frame in.
    next_pts: Option<i64>,
    /// Segments converted on several threads at once; `None` converts as one stream.
    par: Option<Parallel>,
    /// An error met while draining, reported by the next `receive_frame`.
    failed: Option<Error>,
}

/// The least input a segment holds, in frames; rounded up to the ratio's input period.
const SEGMENT_FRAMES: u64 = 32_768;
/// Past this input period, segments would be too long to be worth splitting.
const MAX_PERIOD: u64 = 1 << 18;

/// Converting on several threads. The stream is cut into segments that start on a multiple
/// of the reduced ratio's input period (`down`), so the first output of every segment has
/// phase 0, exactly as in one continuous run. Each segment is converted by a resampler of its
/// own, reset and fed from a pre-roll at least as long as the filter reaches back, and on to
/// a post-roll as long as it reaches forward; its outputs before the segment are dropped. So
/// every output is the same dot product of the same inputs with the same table row, summed in
/// the same order: the bits do not depend on the thread count or on where segments fall.
/// The direct (gather) form only; heavy decimation uses the transposed form, which stays on
/// one stream.
struct Parallel {
    threads: usize,
    workers: Vec<Resampler>,
    up: u64,
    down: u64,
    /// Frames per segment, a multiple of `down`.
    segment: u64,
    /// The filter's reach back and forward, in input frames.
    pre: u64,
    post: u64,
    /// Input frames from `base` on, interleaved.
    pending: Vec<f64>,
    base: u64,
    /// The first input frame not converted yet; a multiple of `down`.
    next: u64,
    received: u64,
    batches: u64,
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// One segment: input frames `start..end`; `last` flushes the stream's tail.
struct Segment {
    start: u64,
    end: u64,
    last: bool,
}

impl Parallel {
    fn new(
        from: u32,
        to: u32,
        channels: usize,
        quality: Quality,
        threads: usize,
        min_segment: u64,
        latency: u64,
    ) -> Result<Option<Self>, Error> {
        let g = gcd(u64::from(from), u64::from(to));
        let (up, down) = (u64::from(to) / g, u64::from(from) / g);
        let gather = down <= up.saturating_mul(crossfade_resample::MAX_GATHER_DECIMATION);
        if threads < 2 || from == to || !gather || down > MAX_PERIOD {
            return Ok(None);
        }
        let workers = (0..threads)
            .map(|_| Resampler::new(from, to, channels, quality.preset()))
            .collect::<Result<_, _>>()
            .map_err(engine_error)?;
        Ok(Some(Self {
            threads,
            workers,
            up,
            down,
            segment: min_segment.div_ceil(down).max(1) * down,
            pre: latency,
            post: latency,
            pending: Vec::new(),
            base: 0,
            next: 0,
            received: 0,
            batches: 0,
        }))
    }

    /// Converts `segments` (consecutive, from `self.next`) and appends their outputs in order.
    fn run(
        &mut self,
        segments: &[Segment],
        channels: usize,
        out: &mut Vec<f64>,
    ) -> Result<(), Error> {
        let (up, down, pre, base) = (self.up, self.down, self.pre, self.base);
        let pending = &self.pending;
        let available = base + (pending.len() / channels) as u64;
        let one = |r: &mut Resampler, seg: &Segment| -> Result<Vec<f64>, Error> {
            // From a pre-roll that starts on the input period, so phases line up.
            let from = (seg.start.saturating_sub(pre) / down) * down;
            let to = if seg.last {
                available
            } else {
                (seg.end + self.post).min(available)
            };
            let slice =
                &pending[((from - base) as usize) * channels..((to - base) as usize) * channels];
            let mut o = Vec::new();
            r.reset();
            r.process(slice, &mut o).map_err(engine_error)?;
            // Near the stream's end the post-roll runs out: what follows is silence, as the
            // continuous run's flush assumes.
            if seg.last || to < seg.end + self.post {
                r.flush(&mut o);
            }
            let skip = ((seg.start - from) * up / down) as usize * channels;
            let take = if seg.last {
                o.len().saturating_sub(skip)
            } else {
                ((seg.end - seg.start) * up / down) as usize * channels
            };
            o.drain(..skip.min(o.len()));
            o.truncate(take);
            Ok(o)
        };
        let results: Vec<Result<Vec<f64>, Error>> = if segments.len() == 1 {
            vec![one(&mut self.workers[0], &segments[0])]
        } else {
            self.batches += 1;
            let (first, rest) = self.workers.split_at_mut(1);
            std::thread::scope(|scope| {
                let handles: Vec<_> = rest
                    .iter_mut()
                    .zip(&segments[1..])
                    .map(|(r, seg)| scope.spawn(|| one(r, seg)))
                    .collect();
                let mut results = vec![one(&mut first[0], &segments[0])];
                for h in handles {
                    results.push(h.join().unwrap_or_else(|_| {
                        Err(Error::Unsupported {
                            what: "resample: a worker thread panicked".into(),
                        })
                    }));
                }
                results
            })
        };
        for r in results {
            out.extend_from_slice(&r?);
        }
        Ok(())
    }

    /// Takes `samples` in and converts every whole batch it completes.
    fn push(&mut self, samples: &[f64], channels: usize, out: &mut Vec<f64>) -> Result<(), Error> {
        self.pending.extend_from_slice(samples);
        self.received += (samples.len() / channels) as u64;
        let batch = self.segment * self.threads as u64;
        while self.received >= self.next + batch + self.post {
            let segments: Vec<Segment> = (0..self.threads as u64)
                .map(|i| Segment {
                    start: self.next + i * self.segment,
                    end: self.next + (i + 1) * self.segment,
                    last: false,
                })
                .collect();
            self.run(&segments, channels, out)?;
            self.next += batch;
            // Keep what the next segment's pre-roll needs.
            let keep = (self.next.saturating_sub(self.pre) / self.down) * self.down;
            let drop = ((keep - self.base) as usize) * channels;
            self.pending.drain(..drop);
            self.base = keep;
        }
        Ok(())
    }

    /// Converts the rest of the stream: on the caller's thread when it is under two segments.
    fn finish(&mut self, channels: usize, out: &mut Vec<f64>) -> Result<(), Error> {
        let rest = self.received - self.next;
        let n = (rest / self.segment).min(self.threads as u64).max(1);
        let n = if rest < 2 * self.segment { 1 } else { n };
        let segments: Vec<Segment> = (0..n)
            .map(|i| Segment {
                start: self.next + i * self.segment,
                end: if i + 1 == n {
                    self.received
                } else {
                    self.next + (i + 1) * self.segment
                },
                last: i + 1 == n,
            })
            .collect();
        self.run(&segments, channels, out)?;
        self.next = self.received;
        self.pending.clear();
        Ok(())
    }
}

fn engine_error(e: crossfade_resample::Error) -> Error {
    Error::Unsupported {
        what: format!("resample: {e}"),
    }
}

/// The threads `0` (the machine's parallelism) stands for.
fn resolve_threads(threads: usize) -> usize {
    match threads {
        0 => std::thread::available_parallelism().map_or(1, |n| n.get().min(8)),
        n => n,
    }
}

impl Resample {
    /// On the calling thread.
    pub fn new(input: &AudioSpec, plan: &ResamplePlan) -> Result<Self, Error> {
        Self::with_threads(input, plan, 1)
    }

    /// On up to `threads` threads (`0`: the machine's parallelism, at most 8; `1` starts
    /// none). The output is the same at every count.
    pub fn with_threads(
        input: &AudioSpec,
        plan: &ResamplePlan,
        threads: usize,
    ) -> Result<Self, Error> {
        Self::with_segment(input, plan, threads, SEGMENT_FRAMES)
    }

    /// [`Self::with_threads`] with segments of at least `min_segment` input frames: tests use
    /// short ones to put many seams in a short stream.
    #[doc(hidden)]
    pub fn with_segment(
        input: &AudioSpec,
        plan: &ResamplePlan,
        threads: usize,
        min_segment: u64,
    ) -> Result<Self, Error> {
        let channels = usize::from(input.layout().channels());
        let resampler = Resampler::new(
            input.sample_rate(),
            plan.rate,
            channels,
            plan.quality.preset(),
        )
        .map_err(engine_error)?;
        let par = Parallel::new(
            input.sample_rate(),
            plan.rate,
            channels,
            plan.quality,
            resolve_threads(threads),
            min_segment,
            resampler.latency(),
        )?;
        Ok(Self {
            spec: AudioSpec::new(plan.rate, input.format(), input.layout().clone())?,
            channels,
            resampler,
            buf: Vec::new(),
            next_pts: None,
            par,
            failed: None,
        })
    }

    /// How many times segments were converted on several threads at once.
    pub fn parallel_batches(&self) -> u64 {
        self.par.as_ref().map_or(0, |p| p.batches)
    }
}

impl Filter for Resample {
    fn output_spec(&self) -> &AudioSpec {
        &self.spec
    }

    fn send_frame(&mut self, frame: &AudioFrame) -> Result<(), Error> {
        let Samples::F64(s) = &frame.samples else {
            return Err(super::not_working_format("resample", frame));
        };
        if self.next_pts.is_none() {
            let at = i128::from(frame.pts) * i128::from(self.spec.sample_rate());
            let pts = at.div_euclid(i128::from(frame.sample_rate));
            self.next_pts = Some(i64::try_from(pts).map_err(|_| Error::Overflow)?);
        }
        if !s.len().is_multiple_of(self.channels) {
            return Err(engine_error(crossfade_resample::Error::PartialFrame {
                samples: s.len(),
                channels: self.channels,
            }));
        }
        match &mut self.par {
            Some(par) => par.push(s, self.channels, &mut self.buf),
            None => self
                .resampler
                .process(s, &mut self.buf)
                .map_err(engine_error),
        }
    }

    fn receive_frame(&mut self, out: &mut AudioFrame) -> Result<bool, Error> {
        if let Some(e) = self.failed.take() {
            return Err(e);
        }
        if self.buf.is_empty() {
            return Ok(false);
        }
        let pts = self.next_pts.unwrap_or(0);
        out.sample_rate = self.spec.sample_rate();
        out.layout.clone_from(self.spec.layout());
        out.pts = pts;
        // Hand the buffer over and keep the frame's old one, so neither side allocates.
        let mut spare = match std::mem::replace(&mut out.samples, Samples::F64(Vec::new())) {
            Samples::F64(v) => v,
            _ => Vec::new(),
        };
        spare.clear();
        std::mem::swap(&mut spare, &mut self.buf);
        out.samples = Samples::F64(spare);
        self.next_pts = Some(pts + out.frames() as i64);
        Ok(true)
    }

    fn drain(&mut self) {
        match &mut self.par {
            Some(par) => {
                if let Err(e) = par.finish(self.channels, &mut self.buf) {
                    self.failed = Some(e);
                }
            }
            None => self.resampler.flush(&mut self.buf),
        }
    }
}
