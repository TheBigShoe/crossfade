//! Opus encoder adapter (stub).

use crate::EncodeOptions;
use crossfade_core::{AudioSpec, Encoder, Error};

pub fn pre_skip() -> u16 {
    0
}

pub fn default_bitrate(_channels: u8) -> u32 {
    0
}

pub fn head(_channels: u8, _input_sample_rate: u32) -> Vec<u8> {
    Vec::new()
}

pub fn encoder(_spec: &AudioSpec, _options: &EncodeOptions) -> Result<Box<dyn Encoder>, Error> {
    Err(Error::Unsupported {
        what: "opus encoding is not implemented".into(),
    })
}

#[cfg(test)]
#[path = "opus_encode_tests.rs"]
mod tests;
