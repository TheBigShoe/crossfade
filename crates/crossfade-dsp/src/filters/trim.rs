//! Trim: keep frames `start..end` of the stream, sample-accurate.
//!
//! Positions count frames from the first one the filter receives; a frame straddling `start`
//! or `end` is cut inside. Output pts follow the `Filter` contract (the first output frame
//! carries the first input pts, then contiguous), so a trimmed file starts where its input did.

use crossfade_core::{AudioFrame, AudioSpec, Error, Filter, Samples};

/// Frame positions at the filter's input rate; `end` is exclusive, `None` runs to the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrimPlan {
    pub start: u64,
    pub end: Option<u64>,
}

impl TrimPlan {
    pub fn output_frames(&self, frames: Option<u64>) -> Option<u64> {
        let n = frames?;
        Some(self.end.map_or(n, |e| e.min(n)).saturating_sub(self.start))
    }
}

pub struct Trim {
    spec: AudioSpec,
    channels: usize,
    start: u64,
    end: u64,
    /// Input frames received so far.
    seen: u64,
    /// Output frames produced so far.
    emitted: u64,
    base: Option<i64>,
    buf: Vec<f64>,
    pts: i64,
    ready: bool,
}

impl Trim {
    pub fn new(input: &AudioSpec, plan: &TrimPlan) -> Self {
        Self {
            spec: input.clone(),
            channels: usize::from(input.layout().channels()),
            start: plan.start,
            end: plan.end.unwrap_or(u64::MAX),
            seen: 0,
            emitted: 0,
            base: None,
            buf: Vec::new(),
            pts: 0,
            ready: false,
        }
    }

    /// True once the filter will emit nothing more, so the caller may stop feeding it.
    pub fn finished(&self) -> bool {
        self.seen >= self.end || self.start >= self.end
    }
}

impl Filter for Trim {
    fn output_spec(&self) -> &AudioSpec {
        &self.spec
    }

    fn send_frame(&mut self, frame: &AudioFrame) -> Result<(), Error> {
        let Samples::F64(s) = &frame.samples else {
            return Err(super::not_working_format("trim", frame));
        };
        let base = *self.base.get_or_insert(frame.pts);
        let n = (s.len() / self.channels) as u64;
        let (first, last) = (self.seen, self.seen.saturating_add(n));
        self.seen = last;
        let from = self.start.max(first);
        let to = self.end.min(last);
        self.buf.clear();
        if from < to {
            let a = ((from - first) as usize) * self.channels;
            let b = ((to - first) as usize) * self.channels;
            self.buf.extend_from_slice(&s[a..b]);
            self.pts = base + self.emitted as i64;
            self.emitted += to - from;
        }
        self.ready = !self.buf.is_empty();
        Ok(())
    }

    fn receive_frame(&mut self, out: &mut AudioFrame) -> Result<bool, Error> {
        if !std::mem::take(&mut self.ready) {
            return Ok(false);
        }
        out.sample_rate = self.spec.sample_rate();
        out.layout.clone_from(self.spec.layout());
        out.pts = self.pts;
        let mut spare = match std::mem::replace(&mut out.samples, Samples::F64(Vec::new())) {
            Samples::F64(v) => v,
            _ => Vec::new(),
        };
        std::mem::swap(&mut spare, &mut self.buf);
        out.samples = Samples::F64(spare);
        Ok(true)
    }

    /// Past its end, nothing more can come out: the pipeline may stop reading.
    fn ignores_further_input(&self) -> bool {
        self.finished()
    }

    fn drain(&mut self) {}
}
