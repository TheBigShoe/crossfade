//! The AAC-LC decoder (ISO/IEC 14496-3 subpart 4, and 13818-7 LC).

use crate::Result;
use crate::asc::AudioSpecificConfig;
use crate::error::unsupported;

pub(crate) mod codebooks;
pub(crate) mod tables;

/// Samples per channel in a frame.
pub const FRAME_LEN: usize = 1024;

/// Decodes raw data blocks into interleaved `f32`.
pub struct Decoder {
    channels: usize,
    sample_rate: u32,
}

impl Decoder {
    pub fn new(config: &AudioSpecificConfig) -> Result<Self> {
        let _ = config;
        Err(unsupported("the decoder is not built yet"))
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn sbr_present(&self) -> bool {
        false
    }

    pub fn decode(&mut self, raw_data_block: &[u8], out: &mut Vec<f32>) -> Result<usize> {
        let _ = (raw_data_block, out);
        Err(unsupported("the decoder is not built yet"))
    }

    pub fn reset(&mut self) {}
}
