//! The ADTS frame header (ISO/IEC 13818-7 and 14496-3 1.A.2.2): seven bytes, nine with the
//! CRC, in front of every frame of a `.aac` file.

use crate::error::{Result, malformed};

/// The sampling rates of `sampling_frequency_index` 0 to 12 (14496-3 Table 1.18).
pub const SAMPLE_RATES: [u32; 13] = [
    96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
];

/// One parsed, validated ADTS header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// The `ID` bit: MPEG-2 AAC (13818-7) rather than MPEG-4.
    pub mpeg2: bool,
    /// False when a 16-bit CRC follows the fixed seven bytes.
    pub protection_absent: bool,
    /// `profile_ObjectType`: the MPEG-4 audio object type minus one (1 is AAC LC).
    pub profile: u8,
    /// Index into [`SAMPLE_RATES`], below 13.
    pub sampling_index: u8,
    /// 0 (a program config element in the stream says) or 1 to 7.
    pub channel_configuration: u8,
    /// The whole frame in bytes, this header included.
    pub frame_length: u16,
    pub buffer_fullness: u16,
    /// Raw data blocks in the frame, 1 to 4.
    pub raw_data_blocks: u8,
}

impl Header {
    /// Bytes [`parse`](Self::parse) needs at least.
    pub const MIN_LEN: usize = 7;

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let _ = bytes;
        Err(malformed("adts: not parsed yet"))
    }

    pub fn header_len(&self) -> usize {
        0
    }

    pub fn payload_len(&self) -> usize {
        0
    }

    pub fn object_type(&self) -> u8 {
        0
    }

    pub fn sample_rate(&self) -> u32 {
        0
    }

    pub fn audio_specific_config(&self) -> [u8; 2] {
        [0; 2]
    }

    pub fn same_stream(&self, _other: &Self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests;
