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
    resampler: Resampler,
    /// Converted samples not yet handed out.
    buf: Vec<f64>,
    /// pts of the next frame out, from the first frame in.
    next_pts: Option<i64>,
}

fn engine_error(e: crossfade_resample::Error) -> Error {
    Error::Unsupported {
        what: format!("resample: {e}"),
    }
}

impl Resample {
    pub fn new(input: &AudioSpec, plan: &ResamplePlan) -> Result<Self, Error> {
        let resampler = Resampler::new(
            input.sample_rate(),
            plan.rate,
            usize::from(input.layout().channels()),
            plan.quality.preset(),
        )
        .map_err(engine_error)?;
        Ok(Self {
            spec: AudioSpec::new(plan.rate, input.format(), input.layout().clone())?,
            resampler,
            buf: Vec::new(),
            next_pts: None,
        })
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
        self.resampler
            .process(s, &mut self.buf)
            .map_err(engine_error)
    }

    fn receive_frame(&mut self, out: &mut AudioFrame) -> Result<bool, Error> {
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
        self.resampler.flush(&mut self.buf);
    }
}
