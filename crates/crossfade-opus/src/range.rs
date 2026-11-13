//! The range decoder of RFC 6716 section 4.1 (libopus `celt/entdec.c`).

pub(crate) const SYM_BITS: u32 = 8;
pub(crate) const SYM_MAX: u32 = (1 << SYM_BITS) - 1;
pub(crate) const CODE_TOP: u32 = 1 << 31;
pub(crate) const CODE_BOT: u32 = CODE_TOP >> SYM_BITS;
pub(crate) const CODE_SHIFT: u32 = 32 - SYM_BITS - 1;
pub(crate) const WINDOW_SIZE: u32 = 32;

/// Bits needed to hold `x` (0 for 0): RFC 6716's `ilog`.
pub(crate) fn ilog(x: u32) -> u32 {
    32 - x.leading_zeros()
}

pub(crate) fn tell_frac(_nbits_total: i32, _rng: u32) -> u32 {
    0
}

/// The range decoder over one frame.
#[derive(Debug, Clone)]
pub struct RangeDecoder<'a> {
    buf: &'a [u8],
}

impl<'a> RangeDecoder<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf }
    }
    pub fn decode(&mut self, _ft: u32) -> u32 {
        0
    }
    pub fn decode_bin(&mut self, _bits: u32) -> u32 {
        0
    }
    pub fn update(&mut self, _fl: u32, _fh: u32, _ft: u32) {}
    pub fn dec_bit_logp(&mut self, _logp: u32) -> bool {
        false
    }
    pub fn dec_icdf(&mut self, _icdf: &[u8], _ftb: u32) -> usize {
        0
    }
    pub fn dec_icdf16(&mut self, _icdf: &[u16], _ftb: u32) -> usize {
        0
    }
    pub fn dec_uint(&mut self, _ft: u32) -> u32 {
        0
    }
    pub fn dec_bits(&mut self, _bits: u32) -> u32 {
        0
    }
    pub fn tell(&self) -> i32 {
        0
    }
    pub fn tell_frac(&self) -> u32 {
        0
    }
    pub fn rng(&self) -> u32 {
        0
    }
    pub fn error(&self) -> bool {
        false
    }
    pub fn storage(&self) -> usize {
        self.buf.len()
    }
    pub fn shrink(&mut self, _bytes: usize) {}
    pub fn range_bytes(&self) -> usize {
        0
    }
}

#[cfg(test)]
#[path = "range_tests.rs"]
mod tests;
