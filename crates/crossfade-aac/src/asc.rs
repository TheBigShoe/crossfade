//! The AudioSpecificConfig (ISO/IEC 14496-3 1.6.2.1) with its GASpecificConfig (4.4.1), and
//! the program config element (4.4.1.1) that a configuration or a raw data block may carry.

use crate::Error;
use crate::bits::BitReader;
use crate::error::{Result, malformed, unsupported};

/// SBR (and PS) signalled in a configuration. The decoder plays only the AAC core: this tells
/// a caller that the stream is HE-AAC and what the full rate would be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sbr {
    /// Signalled explicitly (object type 5 or 29 first) rather than by the backward-compatible
    /// extension after the core configuration.
    pub explicit: bool,
    /// Parametric stereo signalled too (HE-AAC v2).
    pub ps: bool,
    /// The output rate with SBR applied.
    pub sample_rate: u32,
}

/// One parsed AudioSpecificConfig.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioSpecificConfig {
    /// The core's audio object type (2 is AAC LC), after explicit SBR signalling is unwrapped.
    pub object_type: u8,
    /// The core's sampling rate in Hz.
    pub sample_rate: u32,
    /// The table index for the rate: as coded, or for an explicit rate the index whose tables
    /// the standard assigns to it (14496-3 Table 4.82).
    pub sampling_index: u8,
    /// 0 (the program config element says) or 1 to 7.
    pub channel_configuration: u8,
    pub program_config: Option<ProgramConfig>,
    /// 960-sample frames instead of 1024.
    pub frame_length_flag: bool,
    pub depends_on_core_coder: bool,
    pub extension_flag: bool,
    pub sbr: Option<Sbr>,
}

impl AudioSpecificConfig {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let _ = bytes;
        Err(malformed("asc: not parsed yet"))
    }

    /// Output channels: from the channel configuration, or the program config element.
    pub fn channels(&self) -> u16 {
        0
    }
}

/// A front, side or back channel element of a program configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Element {
    /// A channel pair (two channels) rather than a single channel.
    pub is_cpe: bool,
    pub tag: u8,
}

/// A coupling channel element named by a program configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Coupling {
    pub independently_switched: bool,
    pub tag: u8,
}

/// A program config element: which elements make up the program, in output order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramConfig {
    pub element_instance_tag: u8,
    /// The object type minus one, as in ADTS.
    pub profile: u8,
    pub sampling_index: u8,
    pub front: Vec<Element>,
    pub side: Vec<Element>,
    pub back: Vec<Element>,
    pub lfe: Vec<u8>,
    pub assoc_data: Vec<u8>,
    pub coupling: Vec<Coupling>,
    pub comment: Vec<u8>,
}

impl ProgramConfig {
    /// The PCE that opens a raw data block (`payload` starts at the block), if its first
    /// element is one; how an ADTS stream with channel configuration 0 says what it holds.
    pub fn from_raw_data_block(payload: &[u8]) -> Result<Option<Self>> {
        let _ = payload;
        Ok(None)
    }

    /// Output channels: every front, side and back element (two for a pair) and every LFE.
    pub fn channels(&self) -> u16 {
        0
    }
}

#[cfg(test)]
mod tests;
