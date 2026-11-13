//! Opus packets: the TOC byte and frame packing of RFC 6716 section 3.

use crate::error::{Error, Result};

pub const MAX_FRAMES: usize = 48;
pub const MAX_FRAME_LEN: usize = 1275;
pub const MAX_PACKET_SAMPLES: usize = 5760;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    Silk,
    Hybrid,
    Celt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Bandwidth {
    Narrow,
    Medium,
    Wide,
    SuperWide,
    Full,
}

impl Bandwidth {
    pub fn celt_end_band(self) -> usize {
        0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Toc(pub u8);

impl Toc {
    pub fn config(self) -> u8 {
        0
    }
    pub fn mode(self) -> Mode {
        Mode::Silk
    }
    pub fn bandwidth(self) -> Bandwidth {
        Bandwidth::Narrow
    }
    pub fn frame_samples(self) -> usize {
        0
    }
    pub fn stereo(self) -> bool {
        false
    }
    pub fn code(self) -> u8 {
        0
    }
}

#[derive(Debug, Clone)]
pub struct Packet<'a> {
    toc: Toc,
    frames: [&'a [u8]; MAX_FRAMES],
    count: usize,
    padding: &'a [u8],
    byte_len: usize,
}

impl<'a> Packet<'a> {
    pub fn parse(_data: &'a [u8]) -> Result<Self> {
        Err(Error::Unsupported("not yet"))
    }
    pub fn parse_self_delimited(_data: &'a [u8]) -> Result<Self> {
        Err(Error::Unsupported("not yet"))
    }
    pub fn toc(&self) -> Toc {
        self.toc
    }
    pub fn frames(&self) -> &[&'a [u8]] {
        &self.frames[..self.count]
    }
    pub fn padding(&self) -> &'a [u8] {
        self.padding
    }
    pub fn byte_len(&self) -> usize {
        self.byte_len
    }
    pub fn duration(&self) -> usize {
        0
    }
}

pub fn packet_duration(_data: &[u8]) -> Result<usize> {
    Err(Error::Unsupported("not yet"))
}

#[cfg(test)]
#[path = "packet_tests.rs"]
mod tests;
