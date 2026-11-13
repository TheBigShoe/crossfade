use std::fmt;

/// What parsing or decoding can fail with. Non-exhaustive so that a later version can add a
/// case without breaking code that matches on it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The input breaks the syntax or a rule of the standard; the text says which.
    Malformed(String),
    /// Valid input using something this crate does not implement; the text names it.
    Unsupported(String),
    /// The input ended inside a structure.
    UnexpectedEnd,
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(what) | Self::Unsupported(what) => f.write_str(what),
            Self::UnexpectedEnd => f.write_str("aac: input ends inside a structure"),
        }
    }
}

impl std::error::Error for Error {}

pub(crate) fn malformed(what: impl Into<String>) -> Error {
    Error::Malformed(format!("aac: {}", what.into()))
}

pub(crate) fn unsupported(what: impl Into<String>) -> Error {
    Error::Unsupported(format!("aac: {}", what.into()))
}
