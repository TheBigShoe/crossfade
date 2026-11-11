//! Sample-rate conversion, a thin wrapper over `crossfade-resample`.
//!
//! Not built yet: the running filter belongs here (see `build` in the parent module).

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
    /// input length is unknown or the count does not fit.
    pub fn output_frames(&self, input_rate: u32, frames: Option<u64>) -> Option<u64> {
        let n = u128::from(frames?) * u128::from(self.rate);
        u64::try_from(n.div_ceil(u128::from(input_rate)))
            .ok()
            .filter(|&f| f <= i64::MAX as u64)
    }
}
