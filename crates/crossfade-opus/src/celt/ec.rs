//! CELT's view of the frame's range decoder: its calls under the names `celt_decoder.c` uses,
//! with `tell_frac` in the `opus_int32` the reference's callers hold it in.

use crate::range::RangeDecoder;

pub(super) struct Ec<'a, 'b> {
    dec: &'a mut RangeDecoder<'b>,
}

impl<'a, 'b> Ec<'a, 'b> {
    pub(super) fn new(dec: &'a mut RangeDecoder<'b>) -> Self {
        Self { dec }
    }

    pub(super) fn tell(&self) -> i32 {
        self.dec.tell()
    }

    /// `ec_tell_frac` as the reference's callers hold it, in an `opus_int32`.
    pub(super) fn tell_frac(&self) -> i32 {
        self.dec.tell_frac() as i32
    }

    /// Counts every bit up to `total` as read.
    pub(super) fn pretend_read_to(&mut self, total: i32) {
        self.dec.mark_read_to(total);
    }

    /// The frame's length in bytes (`dec->storage`).
    pub(super) fn storage(&self) -> i32 {
        self.dec.storage() as i32
    }

    pub(super) fn bit_logp(&mut self, logp: u32) -> bool {
        self.dec.dec_bit_logp(logp)
    }

    pub(super) fn icdf(&mut self, icdf: &[u8], ftb: u32) -> usize {
        self.dec.dec_icdf(icdf, ftb)
    }

    pub(super) fn uint(&mut self, ft: u32) -> u32 {
        self.dec.dec_uint(ft)
    }

    pub(super) fn bits(&mut self, bits: u32) -> u32 {
        self.dec.dec_bits(bits)
    }

    pub(super) fn decode(&mut self, ft: u32) -> u32 {
        self.dec.decode(ft)
    }

    pub(super) fn decode_bin(&mut self, bits: u32) -> u32 {
        self.dec.decode_bin(bits)
    }

    pub(super) fn update(&mut self, fl: u32, fh: u32, ft: u32) {
        self.dec.update(fl, fh, ft);
    }

    pub(super) fn rng(&self) -> u32 {
        self.dec.rng()
    }
}
