//! The crate's own error type; the crate depends on nothing in the workspace.

use std::fmt;

/// Why a meter could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The rate is outside 8,000 to 1,048,575 Hz.
    SampleRate { rate: u32 },
    /// A meter needs at least one channel.
    NoChannels,
    /// More channels than the meter supports.
    TooManyChannels { count: usize },
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SampleRate { rate } => write!(
                f,
                "sample rate {rate} Hz is outside {}..={} Hz",
                crate::MIN_SAMPLE_RATE,
                crate::MAX_SAMPLE_RATE
            ),
            Self::NoChannels => f.write_str("no channels"),
            Self::TooManyChannels { count } => {
                write!(
                    f,
                    "{count} channels; at most {} are supported",
                    crate::MAX_CHANNELS
                )
            }
        }
    }
}

impl std::error::Error for Error {}
