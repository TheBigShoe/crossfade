//! ALAC encoder adapter: the inverse of the decoder adapter in `alac`.

use crate::EncodeOptions;
use crossfade_core::{AudioSpec, Encoder, Error, SampleFormat};

/// The depth ALAC codes `format`'s samples at, given their true depth (`None`: the format's
/// full width), or `None` when ALAC cannot hold the format.
pub fn coded_depth(_format: SampleFormat, _true_depth: Option<u8>) -> Option<u8> {
    Some(0)
}

pub fn encoder(_spec: &AudioSpec, _options: &EncodeOptions) -> Result<Box<dyn Encoder>, Error> {
    Err(Error::Unsupported {
        what: "alac encoding is not implemented".into(),
    })
}

#[cfg(test)]
#[path = "alac_encode_tests.rs"]
mod tests;
