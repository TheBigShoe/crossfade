//! The Layer III frame decoder. Not built yet.

use crate::{Error, FrameHeader};

/// The longest frame decoded.
pub const MAX_FRAME_LEN: usize = 4096;
/// The output's bound.
pub const OUTPUT_LIMIT: f32 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameInfo {
    pub header: FrameHeader,
    pub samples: usize,
}

#[derive(Default)]
pub struct Decoder;

impl Decoder {
    pub fn new() -> Self {
        Self
    }

    pub fn reset(&mut self) {}

    pub fn decode_frame(&mut self, frame: &[u8], out: &mut Vec<f32>) -> Result<FrameInfo, Error> {
        let _ = (frame, out);
        Err(Error::Corrupt("not built"))
    }
}
