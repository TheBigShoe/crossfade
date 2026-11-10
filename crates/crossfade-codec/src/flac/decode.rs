//! FLAC decoder.

use crossfade_core::{Decoder, Error, StreamInfo};

pub fn decoder(_stream: &StreamInfo) -> Result<Box<dyn Decoder>, Error> {
    Err(Error::Unsupported {
        what: "flac decoding is not built yet".into(),
    })
}

#[cfg(test)]
#[path = "decode_tests.rs"]
mod tests;
