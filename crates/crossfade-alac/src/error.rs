use std::fmt;

/// What went wrong. Every message names ALAC, so a caller can pass it on as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The ALACSpecificConfig is not one this decoder can use.
    InvalidConfig(&'static str),
    /// A packet breaks the format.
    Malformed(&'static str),
    /// A packet uses something the format allows but this decoder does not decode.
    Unsupported(&'static str),
    /// A packet ended in the middle of something.
    UnexpectedEnd,
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(what) => write!(f, "alac: invalid config: {what}"),
            Self::Malformed(what) => write!(f, "alac: {what}"),
            Self::Unsupported(what) => write!(f, "alac: {what} is not supported"),
            Self::UnexpectedEnd => f.write_str("alac: packet ends too soon"),
        }
    }
}

impl std::error::Error for Error {}
