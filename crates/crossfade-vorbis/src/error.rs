//! The crate's own error type; the crate depends on nothing, so engines map it at their edge.

use std::fmt;

/// What parsing headers or decoding audio can fail with. Non-exhaustive so that a later version
/// can add a case without breaking code that matches on it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// A header breaks the format (or holds a value this decoder refuses as degenerate). The
    /// stream cannot be decoded.
    Malformed(String),
    /// A header needs more memory than the caller allows.
    OverBudget { requested: usize, budget: usize },
    /// One audio packet cannot be decoded. Nothing changed: the decoder carries on with the next
    /// packet as if this one had not been there.
    BadPacket(&'static str),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(what) => write!(f, "vorbis: {what}"),
            Self::OverBudget { requested, budget } => write!(
                f,
                "vorbis: the headers need {requested} bytes, more than the {budget} allowed"
            ),
            Self::BadPacket(what) => write!(f, "vorbis: bad audio packet: {what}"),
        }
    }
}

impl std::error::Error for Error {}

pub(crate) fn malformed(what: impl Into<String>) -> Error {
    Error::Malformed(what.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_texts_name_the_codec() {
        assert_eq!(malformed("x").to_string(), "vorbis: x");
        assert_eq!(
            Error::OverBudget {
                requested: 9,
                budget: 8
            }
            .to_string(),
            "vorbis: the headers need 9 bytes, more than the 8 allowed"
        );
        assert_eq!(
            Error::BadPacket("y").to_string(),
            "vorbis: bad audio packet: y"
        );
    }
}
