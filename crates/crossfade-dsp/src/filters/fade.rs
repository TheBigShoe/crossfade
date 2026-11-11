//! Fades: in from silence at the start, out to silence at the end, along a named curve.
//!
//! A fade of `n` frames gives frame `k` the gain `curve(k / n)`, `k` an integer (never
//! accumulated, so chunking cannot change a bit). A fade-in counts `k` from its first frame
//! (silent) up to `n - 1`; the frame after it is untouched. A fade-out is the mirror image:
//! `k` counts back from the last frame, which is silent.
//!
//! A fade-out needs the stream's end, which is not known in advance, so the filter holds
//! back exactly `fade_out` frames (`fade_out * channels * 8` bytes) and releases older ones
//! as newer arrive; `drain` fades and releases what it holds. For that reason a fade-out
//! longer than [`MAX_FADE_OUT_SECONDS`] at the input rate is refused. A stream shorter than
//! `fade_in + fade_out` gets both gains, multiplied; one shorter than `fade_out` is the tail
//! of the fade-out (so its first frame is not silent).

use crossfade_core::{AudioFrame, AudioSpec, Error, Filter, Samples};
use std::collections::VecDeque;

/// The longest fade-out accepted, in seconds at the input rate.
pub const MAX_FADE_OUT_SECONDS: u64 = 600;

/// Frames per output frame while draining the held tail.
const DRAIN_CHUNK: usize = 8192;

/// The gain shape over a fade's length, `t` from 0 to 1 (a fade-out runs it backwards).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Curve {
    /// `g = t`.
    #[default]
    Linear,
    /// `g = sin(t * pi / 2)`: constant power when crossfaded with its mirror.
    EqualPower,
    /// `g = (1 - cos(t * pi)) / 2`: gentle at both ends.
    SCurve,
}

impl Curve {
    pub const NAMES: [&'static str; 3] = ["linear", "equal-power", "s-curve"];

    pub fn name(self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::EqualPower => "equal-power",
            Self::SCurve => "s-curve",
        }
    }

    /// The gain at `t = k / n`, for `k` in `0..n`.
    pub fn gain(self, k: u64, n: u64) -> f64 {
        let t = k as f64 / n as f64;
        match self {
            Self::Linear => t,
            Self::EqualPower => libm::sin(t * std::f64::consts::FRAC_PI_2),
            Self::SCurve => (1.0 - libm::cos(t * std::f64::consts::PI)) / 2.0,
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "linear" => Some(Self::Linear),
            "equal-power" => Some(Self::EqualPower),
            "s-curve" => Some(Self::SCurve),
            _ => None,
        }
    }
}

/// Lengths in frames at the filter's input rate; 0 means no fade at that end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FadePlan {
    pub fade_in: u64,
    pub fade_out: u64,
    pub curve: Curve,
}

pub struct Fade {
    spec: AudioSpec,
    channels: usize,
    plan: FadePlan,
    /// Frames received so far.
    seen: u64,
    /// Frames released so far.
    emitted: u64,
    base: Option<i64>,
    held: VecDeque<f64>,
    draining: bool,
    buf: Vec<f64>,
}

impl Fade {
    pub fn new(input: &AudioSpec, plan: &FadePlan) -> Result<Self, Error> {
        let rate = u64::from(input.sample_rate());
        if plan.fade_out > MAX_FADE_OUT_SECONDS * rate {
            return Err(Error::Unsupported {
                what: format!(
                    "a fade-out longer than {MAX_FADE_OUT_SECONDS} seconds ({} frames at {rate} Hz) \
                     would hold too much audio in memory",
                    plan.fade_out
                ),
            });
        }
        let channels = usize::from(input.layout().channels());
        let held_cap = usize::try_from(plan.fade_out)
            .unwrap_or(usize::MAX)
            .saturating_mul(channels);
        Ok(Self {
            spec: input.clone(),
            channels,
            plan: *plan,
            seen: 0,
            emitted: 0,
            base: None,
            held: VecDeque::with_capacity(held_cap.min(1 << 20)),
            draining: false,
            buf: Vec::new(),
        })
    }
}

impl Filter for Fade {
    fn output_spec(&self) -> &AudioSpec {
        &self.spec
    }

    fn send_frame(&mut self, frame: &AudioFrame) -> Result<(), Error> {
        let Samples::F64(s) = &frame.samples else {
            return Err(super::not_working_format("fade", frame));
        };
        self.base.get_or_insert(frame.pts);
        let (n_in, curve) = (self.plan.fade_in, self.plan.curve);
        for f in s.chunks_exact(self.channels) {
            if self.seen < n_in {
                let g = curve.gain(self.seen, n_in);
                self.held.extend(f.iter().map(|&x| x * g));
            } else {
                self.held.extend(f.iter().copied());
            }
            self.seen += 1;
        }
        Ok(())
    }

    fn receive_frame(&mut self, out: &mut AudioFrame) -> Result<bool, Error> {
        let keep = if self.draining {
            0
        } else {
            self.plan.fade_out.min(usize::MAX as u64) as usize
        };
        let held = self.held.len() / self.channels;
        let mut frames = held.saturating_sub(keep);
        if self.draining {
            frames = frames.min(DRAIN_CHUNK);
        }
        if frames == 0 {
            return Ok(false);
        }
        self.buf.clear();
        let n_out = self.plan.fade_out;
        let curve = self.plan.curve;
        for _ in 0..frames {
            // Distance from the last frame; only meaningful once draining (the end is known).
            let g = if self.draining {
                let k = self.seen - 1 - self.emitted;
                (k < n_out).then(|| curve.gain(k, n_out))
            } else {
                None
            };
            for x in self.held.drain(..self.channels) {
                self.buf.push(g.map_or(x, |g| x * g));
            }
            self.emitted += 1;
        }
        out.sample_rate = self.spec.sample_rate();
        out.layout.clone_from(self.spec.layout());
        out.pts = self.base.unwrap_or(0) + (self.emitted - frames as u64) as i64;
        let mut spare = match std::mem::replace(&mut out.samples, Samples::F64(Vec::new())) {
            Samples::F64(v) => v,
            _ => Vec::new(),
        };
        std::mem::swap(&mut spare, &mut self.buf);
        out.samples = Samples::F64(spare);
        Ok(true)
    }

    fn drain(&mut self) {
        self.draining = true;
    }
}
