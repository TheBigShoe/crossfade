use crate::Channel;

/// Everything `crossfade-core` can fail with.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("denominator is zero")]
    ZeroDenominator,
    #[error("division by zero")]
    DivisionByZero,
    #[error("arithmetic overflow")]
    Overflow,
    #[error("time base must be strictly positive")]
    InvalidTimeBase,
    #[error("channel layout has no channels")]
    EmptyChannelLayout,
    #[error("channel {0:?} appears twice in the layout")]
    DuplicateChannel(Channel),
    #[error("sample rate must be non-zero")]
    ZeroSampleRate,
    #[error("allocation of {requested} bytes exceeds the limit of {max}")]
    AllocLimitExceeded { requested: usize, max: usize },
    #[error("i/o error: {message}")]
    Io {
        kind: std::io::ErrorKind,
        message: String,
    },
    #[error("unexpected end of input at byte {offset}")]
    UnexpectedEof { offset: u64 },
    #[error("malformed input: {what}")]
    Malformed { what: String },
    #[error("unsupported: {what}")]
    Unsupported { what: String },
    #[error("unrecognised format")]
    UnrecognisedFormat,
    #[error("cancelled")]
    Cancelled,
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io {
            kind: e.kind(),
            message: e.to_string(),
        }
    }
}
