//! The Ogg Opus header packets (RFC 7845 section 5).

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelMapping {
    pub family: u8,
    pub streams: u8,
    pub coupled: u8,
    pub table: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpusHead {
    pub version: u8,
    pub channels: u8,
    pub pre_skip: u16,
    pub input_sample_rate: u32,
    pub output_gain: i16,
    pub mapping: ChannelMapping,
}

impl OpusHead {
    pub fn parse(_data: &[u8]) -> Result<Self> {
        Err(Error::Unsupported("not yet"))
    }
    pub fn output_gain_db(&self) -> f64 {
        0.0
    }
    pub fn output_gain_linear(&self) -> f64 {
        0.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpusTags<'a> {
    pub vendor: &'a [u8],
    pub comment_count: u32,
    pub body: &'a [u8],
    pub extra: &'a [u8],
}

impl<'a> OpusTags<'a> {
    pub fn parse(_data: &'a [u8]) -> Result<Self> {
        Err(Error::Unsupported("not yet"))
    }
}

#[cfg(test)]
#[path = "head_tests.rs"]
mod tests;
