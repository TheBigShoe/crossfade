//! The top-level Opus decoder.

use crate::error::{Error, Result};

#[derive(Debug, Clone)]
pub struct Decoder {
    channels: usize,
}

impl Decoder {
    pub fn new(channels: usize) -> Result<Self> {
        Ok(Self { channels })
    }
    pub fn channels(&self) -> usize {
        self.channels
    }
    pub fn final_range(&self) -> u32 {
        1
    }
    pub fn reset(&mut self) {}
    pub fn decode(&mut self, _packet: Option<&[u8]>, _out: &mut Vec<f32>) -> Result<usize> {
        Err(Error::Unsupported("not yet"))
    }
}

#[cfg(test)]
#[path = "decoder_tests.rs"]
mod tests;
