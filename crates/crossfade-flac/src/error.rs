//! The crate's own error type; the crate depends on nothing, so engines map it at their edge.

use std::fmt;

/// What parsing or decoding FLAC can fail with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The data breaks the format.
    Malformed(String),
    /// Valid FLAC this crate does not handle (parameters that change mid-stream).
    Unsupported(String),
    /// The input ended early; `offset` is the length of what was available, in bytes.
    UnexpectedEof { offset: usize },
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(what) => write!(f, "malformed input: {what}"),
            Self::Unsupported(what) => write!(f, "unsupported: {what}"),
            Self::UnexpectedEof { offset } => write!(f, "unexpected end of input at byte {offset}"),
        }
    }
}

impl std::error::Error for Error {}
