//! The crate's own error type; the crate depends on nothing but `libm`, so engines map it at
//! their edge.

use crate::header::Layer;
use std::fmt;

/// What parsing or decoding MPEG audio can fail with. Non-exhaustive so that a later version
/// can add a case without breaking code that matches on it.
///
/// [`Decoder::decode_frame`](crate::Decoder::decode_frame) still appends a frame of silence
/// for every error but [`Error::NotAHeader`], so a caller that counts samples stays on time;
/// [`Error::is_reservoir`] tells the one error that is normal at the start of a stream or after
/// a reset from the others.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The four bytes are not an MPEG audio frame header; the text says which field is wrong.
    NotAHeader(&'static str),
    /// A Layer I or Layer II frame: its header parses, its audio is not decoded.
    UnsupportedLayer(Layer),
    /// The frame handed to the decoder is not as long as its header says (`expected` is the
    /// least a free-format frame can be).
    FrameLength { expected: usize, actual: usize },
    /// The frame's CRC-16 does not match its header and side information.
    Crc { expected: u16, actual: u16 },
    /// The frame's side information or main data breaks the format.
    Corrupt(&'static str),
    /// The frame's main data starts `needed` bytes back in the bit reservoir, which holds only
    /// `available`: normal for the first frames after the start of a stream or a reset.
    Reservoir { needed: usize, available: usize },
}

impl Error {
    /// True for [`Error::Reservoir`], the error a decoder gives while it fills its bit
    /// reservoir: a caller that started mid-stream expects it and ignores it.
    pub fn is_reservoir(&self) -> bool {
        matches!(self, Self::Reservoir { .. })
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAHeader(what) => write!(f, "mp3: not a frame header: {what}"),
            Self::UnsupportedLayer(layer) => {
                write!(f, "mp3: MPEG audio layer {} is not supported", layer.name())
            }
            Self::FrameLength { expected, actual } => write!(
                f,
                "mp3: the frame is {actual} bytes, its header says {expected}"
            ),
            Self::Crc { expected, actual } => write!(
                f,
                "mp3: CRC mismatch (frame says {expected:04X}, data gives {actual:04X})"
            ),
            Self::Corrupt(what) => write!(f, "mp3: corrupt frame: {what}"),
            Self::Reservoir { needed, available } => write!(
                f,
                "mp3: the frame needs {needed} bytes of earlier main data, {available} are held"
            ),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_name_the_codec_and_the_problem() {
        assert_eq!(
            Error::NotAHeader("sync").to_string(),
            "mp3: not a frame header: sync"
        );
        assert_eq!(
            Error::UnsupportedLayer(Layer::II).to_string(),
            "mp3: MPEG audio layer II is not supported"
        );
        assert_eq!(
            Error::FrameLength {
                expected: 418,
                actual: 417
            }
            .to_string(),
            "mp3: the frame is 417 bytes, its header says 418"
        );
        assert_eq!(
            Error::Crc {
                expected: 0xBEEF,
                actual: 0x0001
            }
            .to_string(),
            "mp3: CRC mismatch (frame says BEEF, data gives 0001)"
        );
        assert_eq!(Error::Corrupt("x").to_string(), "mp3: corrupt frame: x");
        let r = Error::Reservoir {
            needed: 300,
            available: 0,
        };
        assert_eq!(
            r.to_string(),
            "mp3: the frame needs 300 bytes of earlier main data, 0 are held"
        );
        assert!(r.is_reservoir());
        assert!(!Error::Corrupt("x").is_reservoir());
    }
}
