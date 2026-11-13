//! The crate's own error type; the crate depends on nothing, so engines map it at their edge.

use std::fmt;

/// What parsing or decoding Opus can fail with. Non-exhaustive so that a later version can add
/// a case without breaking code that matches on it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// A packet breaks the framing rules of RFC 6716 section 3 (R1-R7, or Appendix B). It must
    /// not be decoded; the decoder's state is unchanged.
    InvalidPacket(&'static str),
    /// An Ogg Opus header (`OpusHead`, `OpusTags`) breaks RFC 7845 section 5.
    InvalidHeader(&'static str),
    /// A caller's argument is out of range (a channel count, say).
    BadArgument(&'static str),
    /// Something this version cannot do yet, named.
    Unsupported(&'static str),
    /// A frame's coded data is inconsistent (the reference decoder's internal error).
    Corrupt(&'static str),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPacket(what) => write!(f, "opus: invalid packet: {what}"),
            Self::InvalidHeader(what) => write!(f, "opus: invalid header: {what}"),
            Self::BadArgument(what) => write!(f, "opus: bad argument: {what}"),
            Self::Unsupported(what) => write!(f, "opus: {what}"),
            Self::Corrupt(what) => write!(f, "opus: corrupt frame: {what}"),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_texts_name_the_codec() {
        assert_eq!(
            Error::InvalidPacket("x").to_string(),
            "opus: invalid packet: x"
        );
        assert_eq!(
            Error::InvalidHeader("x").to_string(),
            "opus: invalid header: x"
        );
        assert_eq!(Error::BadArgument("x").to_string(), "opus: bad argument: x");
        assert_eq!(
            Error::Unsupported("celt decoding is not built yet").to_string(),
            "opus: celt decoding is not built yet"
        );
        assert_eq!(Error::Corrupt("x").to_string(), "opus: corrupt frame: x");
    }
}
