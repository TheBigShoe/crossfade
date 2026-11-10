//! FLAC encoder.

use crate::EncodeOptions;
use crossfade_core::{AudioSpec, Encoder, Error};

pub fn encoder(_spec: &AudioSpec, _options: &EncodeOptions) -> Result<Box<dyn Encoder>, Error> {
    Err(Error::Unsupported {
        what: "flac encoding is not built yet".into(),
    })
}

#[cfg(test)]
mod tests;
