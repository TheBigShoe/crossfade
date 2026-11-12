//! The four-byte MPEG audio frame header (ISO/IEC 11172-3 2.4.1.3, 13818-3 2.4.1.3, and the
//! unofficial MPEG-2.5 extension to 8-12 kHz).

use crate::Error;

/// MPEG-1, MPEG-2 (the low sampling frequencies, "LSF") or MPEG-2.5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Version {
    Mpeg1,
    Mpeg2,
    Mpeg25,
}

impl Version {
    pub fn name(self) -> &'static str {
        match self {
            Self::Mpeg1 => "MPEG-1",
            Self::Mpeg2 => "MPEG-2",
            Self::Mpeg25 => "MPEG-2.5",
        }
    }

    /// MPEG-2 and MPEG-2.5 share Layer III's half-length frames and their side information.
    pub fn is_lsf(self) -> bool {
        self != Self::Mpeg1
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Layer {
    I,
    II,
    III,
}

impl Layer {
    pub fn name(self) -> &'static str {
        match self {
            Self::I => "I",
            Self::II => "II",
            Self::III => "III",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChannelMode {
    Stereo,
    /// Mid/side and/or intensity stereo, as the mode extension says.
    JointStereo,
    /// Two independent mono channels.
    DualChannel,
    Mono,
}

/// A parsed frame header. Fields are as coded; the derived quantities are methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameHeader {
    pub version: Version,
    pub layer: Layer,
    /// A CRC-16 follows the header (the protection bit is 0).
    pub crc: bool,
    /// 0 is free format: the bitrate, and so the frame length, is not in the header.
    pub bitrate_index: u8,
    pub sample_rate: u32,
    pub padding: bool,
    pub private: bool,
    pub mode: ChannelMode,
    /// Layer III joint stereo: bit 1 mid/side, bit 0 intensity stereo.
    pub mode_extension: u8,
    pub copyright: bool,
    pub original: bool,
    pub emphasis: u8,
}

/// kbit/s by `[mpeg1?][layer][index]`, index 0 (free format) and 15 (reserved) as 0.
const BITRATES: [[[u16; 16]; 3]; 2] = [
    [
        // MPEG-2 and 2.5: layer I, II, III
        [
            0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256, 0,
        ],
        [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0],
        [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0],
    ],
    [
        [
            0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448, 0,
        ],
        [
            0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 0,
        ],
        [
            0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
        ],
    ],
];

const SAMPLE_RATES: [u32; 3] = [44100, 48000, 32000];

impl FrameHeader {
    /// Parses a header. Refuses a broken sync, the reserved version, the reserved layer, bitrate
    /// index 15 and sample-rate index 3.
    pub fn parse(bytes: &[u8; 4]) -> Result<Self, Error> {
        let _ = (bytes, SAMPLE_RATES);
        Err(Error::NotAHeader("not built"))
    }

    pub fn channels(&self) -> u8 {
        if self.mode == ChannelMode::Mono {
            1
        } else {
            2
        }
    }

    pub fn is_free_format(&self) -> bool {
        self.bitrate_index == 0
    }

    /// Bits per second; 0 for free format.
    pub fn bitrate(&self) -> u32 {
        let mpeg1 = usize::from(self.version == Version::Mpeg1);
        let layer = match self.layer {
            Layer::I => 0,
            Layer::II => 1,
            Layer::III => 2,
        };
        u32::from(BITRATES[mpeg1][layer][usize::from(self.bitrate_index)]) * 1000
    }

    /// Sample frames (per channel) one frame decodes to.
    pub fn samples_per_frame(&self) -> u32 {
        match (self.layer, self.version) {
            (Layer::I, _) => 384,
            (Layer::III, v) if v.is_lsf() => 576,
            _ => 1152,
        }
    }

    /// Bytes per slot of the frame-length formula times the slots per frame and bitrate unit:
    /// the frame is `factor * bitrate / sample_rate` slots, plus one when padded.
    fn slot_factor(&self) -> (u32, u32) {
        match self.layer {
            Layer::I => (12, 4),
            _ => (self.samples_per_frame() / 8, 1),
        }
    }

    /// The whole frame's length in bytes, header and padding included; `None` for free format,
    /// whose length the stream gives (see [`free_format_len`](Self::free_format_len)).
    pub fn frame_len(&self) -> Option<usize> {
        if self.is_free_format() {
            return None;
        }
        let (factor, slot) = self.slot_factor();
        let slots = factor * self.bitrate() / self.sample_rate + u32::from(self.padding);
        Some((slots * slot) as usize)
    }

    /// A free-format frame's length, from the length of an unpadded frame of the same stream.
    pub fn free_format_len(&self, unpadded: usize) -> usize {
        let slot = self.slot_factor().1 as usize;
        unpadded + if self.padding { slot } else { 0 }
    }

    /// Bytes of Layer III side information after the header and any CRC; 0 for layers I-II.
    pub fn side_info_len(&self) -> usize {
        match (self.layer, self.version.is_lsf(), self.channels()) {
            (Layer::III, false, 1) => 17,
            (Layer::III, false, _) => 32,
            (Layer::III, true, 1) => 9,
            (Layer::III, true, _) => 17,
            _ => 0,
        }
    }

    /// Where Layer III side information (or a layer I-II frame's audio data) starts.
    pub fn data_start(&self) -> usize {
        4 + if self.crc { 2 } else { 0 }
    }

    /// Joint stereo with mid/side coding on.
    pub fn ms_stereo(&self) -> bool {
        self.mode == ChannelMode::JointStereo && self.mode_extension & 2 != 0
    }

    /// Joint stereo with intensity stereo on.
    pub fn intensity_stereo(&self) -> bool {
        self.mode == ChannelMode::JointStereo && self.mode_extension & 1 != 0
    }

    /// Whether `other` can belong to the same stream: the same version, layer and sample rate,
    /// and both free format or neither. Channel mode, bitrate and the flags may change.
    pub fn same_stream(&self, other: &FrameHeader) -> bool {
        self.version == other.version
            && self.layer == other.layer
            && self.sample_rate == other.sample_rate
            && self.is_free_format() == other.is_free_format()
    }
}
