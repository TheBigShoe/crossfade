//! CELT's view of the frame's range coder: the decoder's calls under the names
//! `celt_decoder.c` uses and the encoder's under `celt_encoder.c`'s, with `tell_frac` in the
//! `opus_int32` the reference's callers hold it in.

use crate::range::RangeDecoder;
use crate::range_encoder::RangeEncoder;

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

/// The frame's range encoder as `celt_encoder.c` calls it.
pub(super) struct Enc<'a> {
    enc: &'a mut RangeEncoder,
}

#[allow(dead_code)] // ENCODER-WIP: until celt::encoder calls all of it
impl<'a> Enc<'a> {
    pub(super) fn new(enc: &'a mut RangeEncoder) -> Self {
        Self { enc }
    }

    pub(super) fn tell(&self) -> i32 {
        self.enc.tell()
    }

    pub(super) fn tell_frac(&self) -> i32 {
        self.enc.tell_frac() as i32
    }

    /// The frame's size in bytes (`enc->storage`).
    pub(super) fn storage(&self) -> i32 {
        self.enc.storage() as i32
    }

    pub(super) fn rng(&self) -> u32 {
        self.enc.rng()
    }

    pub(super) fn bit_logp(&mut self, val: bool, logp: u32) {
        self.enc.enc_bit_logp(val, logp);
    }

    pub(super) fn icdf(&mut self, s: usize, icdf: &[u8], ftb: u32) {
        self.enc.enc_icdf(s, icdf, ftb);
    }

    pub(super) fn uint(&mut self, val: u32, ft: u32) {
        self.enc.enc_uint(val, ft);
    }

    pub(super) fn bits(&mut self, val: u32, bits: u32) {
        self.enc.enc_bits(val, bits);
    }

    pub(super) fn encode(&mut self, fl: u32, fh: u32, ft: u32) {
        self.enc.encode(fl, fh, ft);
    }

    pub(super) fn encode_bin(&mut self, fl: u32, fh: u32, bits: u32) {
        self.enc.encode_bin(fl, fh, bits);
    }

    pub(super) fn shrink(&mut self, bytes: usize) {
        self.enc.shrink(bytes);
    }

    /// The coder's whole state, to try a coding and take it back (`*enc` saved by value in
    /// the reference).
    pub(super) fn save(&self) -> RangeEncoder {
        self.enc.clone()
    }

    pub(super) fn restore(&mut self, saved: RangeEncoder) {
        *self.enc = saved;
    }
}
