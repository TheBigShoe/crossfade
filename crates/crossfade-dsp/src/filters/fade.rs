//! Fades: in from silence at the start, out to silence at the end, along a named curve.
//!
//! Not built yet: the running filter belongs here (see `build` in the parent module).

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
