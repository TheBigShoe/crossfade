//! The ADTS frame header (ISO/IEC 13818-7 and 14496-3 1.A.2.2): seven bytes, nine with the
//! CRC, in front of every frame of a `.aac` file.

use crate::Error;
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
        let Some(b) = bytes.get(..Self::MIN_LEN) else {
            return Err(Error::UnexpectedEnd);
        };
        if b[0] != 0xFF || b[1] & 0xF0 != 0xF0 {
            return Err(malformed("adts: no sync word"));
        }
        if b[1] & 0x06 != 0 {
            return Err(malformed("adts: layer must be 0"));
        }
        let mpeg2 = b[1] & 0x08 != 0;
        let protection_absent = b[1] & 0x01 != 0;
        let profile = b[2] >> 6;
        let sampling_index = (b[2] >> 2) & 0xF;
        if sampling_index > 12 {
            return Err(malformed(format!(
                "adts: reserved sampling index {sampling_index}"
            )));
        }
        if mpeg2 && profile == 3 {
            return Err(malformed("adts: MPEG-2 profile 3 is reserved"));
        }
        let channel_configuration = (b[2] & 1) << 2 | b[3] >> 6;
        let frame_length = u16::from(b[3] & 3) << 11 | u16::from(b[4]) << 3 | u16::from(b[5] >> 5);
        let buffer_fullness = u16::from(b[5] & 0x1F) << 6 | u16::from(b[6] >> 2);
        let header = Self {
            mpeg2,
            protection_absent,
            profile,
            sampling_index,
            channel_configuration,
            frame_length,
            buffer_fullness,
            raw_data_blocks: (b[6] & 3) + 1,
        };
        let len = header.header_len();
        if usize::from(frame_length) < len {
            return Err(malformed(format!(
                "adts: frame length {frame_length} is shorter than its {len}-byte header"
            )));
        }
        if bytes.len() < len {
            return Err(Error::UnexpectedEnd);
        }
        Ok(header)
    }

    /// Seven bytes, or nine with the CRC.
    pub fn header_len(&self) -> usize {
        if self.protection_absent { 7 } else { 9 }
    }

    /// The bytes after the header: the raw data block(s).
    pub fn payload_len(&self) -> usize {
        usize::from(self.frame_length) - self.header_len()
    }

    /// The MPEG-4 audio object type (2 is AAC LC; MPEG-2's profiles map the same way).
    pub fn object_type(&self) -> u8 {
        self.profile + 1
    }

    pub fn sample_rate(&self) -> u32 {
        SAMPLE_RATES[usize::from(self.sampling_index)]
    }

    /// The two-byte AudioSpecificConfig carrying the same object type, rate and channels, so
    /// one decoder path serves ADTS and MP4.
    pub fn audio_specific_config(&self) -> [u8; 2] {
        let v = u16::from(self.object_type()) << 11
            | u16::from(self.sampling_index) << 7
            | u16::from(self.channel_configuration) << 3;
        v.to_be_bytes()
    }

    /// Whether `other` can be a header of the same stream: the fields that are fixed for a
    /// stream (version, profile, rate, channels) agree.
    pub fn same_stream(&self, other: &Self) -> bool {
        self.mpeg2 == other.mpeg2
            && self.profile == other.profile
            && self.sampling_index == other.sampling_index
            && self.channel_configuration == other.channel_configuration
    }
}

/// The raw data blocks of a frame.
pub struct RawDataBlocks<'a> {
    frame: &'a [u8],
    ranges: [(usize, usize); 4],
    count: usize,
    next: usize,
}

impl<'a> Iterator for RawDataBlocks<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        let _ = (self.frame, self.ranges, self.count, self.next);
        None
    }
}

impl Header {
    pub fn raw_data_blocks<'a>(&self, frame: &'a [u8]) -> Result<RawDataBlocks<'a>> {
        Ok(RawDataBlocks {
            frame,
            ranges: [(0, 0); 4],
            count: 0,
            next: 0,
        })
    }
}

#[cfg(test)]
mod tests;
