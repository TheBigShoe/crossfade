//! The crate's own error type; the crate depends on nothing in the workspace, so the engine
//! maps it at its edge.

use std::fmt;

/// What constructing or feeding a [`crate::Resampler`] can fail with. Non-exhaustive so that a
/// later version can add a case without breaking code that matches on it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// A sample rate of 0 Hz or above [`crate::MAX_RATE`].
    InvalidRate(u32),
    /// A channel count of 0 or above [`crate::MAX_CHANNELS`].
    InvalidChannels(usize),
    /// Interleaved input whose length is not a multiple of the channel count. Nothing of the
    /// call was consumed.
    PartialFrame { samples: usize, channels: usize },
    /// A planar call with a different number of channel slices or vectors.
    ChannelMismatch { expected: usize, got: usize },
    /// A planar call whose channel slices are not all the same length.
    UnequalLengths,
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRate(rate) => write!(
                f,
                "sample rate {rate} Hz is outside 1 to {} Hz",
                crate::MAX_RATE
            ),
            Self::InvalidChannels(n) => write!(
                f,
                "channel count {n} is outside 1 to {}",
                crate::MAX_CHANNELS
            ),
            Self::PartialFrame { samples, channels } => write!(
                f,
                "{samples} samples are not whole frames of {channels} channels"
            ),
            Self::ChannelMismatch { expected, got } => write!(
                f,
                "planar call has {got} channels, the resampler {expected}"
            ),
            Self::UnequalLengths => write!(f, "planar input channels differ in length"),
        }
    }
}

impl std::error::Error for Error {}
