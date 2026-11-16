//! CELT's view of the frame's range coder: the decoder's calls under the names
//! `celt_decoder.c` uses and the encoder's under `celt_encoder.c`'s, with `tell_frac` in the
//! `opus_int32` the reference's callers hold it in.

use super::maths::isqrt32;
use super::vq::{alg_quant, alg_unquant};
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

    pub(super) fn mark_written_to(&mut self, bits: i32) {
        self.enc.mark_written_to(bits);
    }

    pub(super) fn error(&self) -> bool {
        self.enc.error()
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

/// What the code encoder and decoder share (`rate.c`'s allocation, `bands.c`'s shapes) asks
/// of either range coder, as the reference's `if (encode) ec_enc_x(..) else x = ec_dec_x(..)`:
/// each call codes `val` (the encoder) or reads it (the decoder, which ignores `val`), and
/// returns the value coded.
pub(super) trait Coder {
    const ENCODE: bool;
    fn tell_frac(&self) -> i32;
    fn code_bit_logp(&mut self, val: bool, logp: u32) -> bool;
    fn code_uint(&mut self, val: u32, ft: u32) -> u32;
    fn code_bits(&mut self, val: u32, bits: u32) -> u32;
    /// A stereo split angle of `0..=qn`: probability 3 up to the middle, 1 after.
    fn code_theta_step(&mut self, itheta: i32, qn: i32) -> i32;
    /// A mono split angle of `0..=qn`: a triangular distribution.
    fn code_theta_triangle(&mut self, itheta: i32, qn: i32) -> i32;
    /// `k` pulses for the shape `x` over `b` blocks (`alg_quant`, `alg_unquant`): `x` becomes
    /// the decoded shape scaled by `gain`; returns the collapse mask.
    fn code_pvq(&mut self, x: &mut [f32], k: usize, spread: i32, b: usize, gain: f32) -> u32;
}

/// The step distribution's symbol for angle `x`: `(fl, fh, ft)`.
fn theta_step_symbol(x: i32, qn: i32) -> (u32, u32, u32) {
    let p0 = 3;
    let x0 = qn / 2;
    let ft = p0 * (x0 + 1) + x0;
    let (fl, fh) = if x <= x0 {
        (p0 * x, p0 * (x + 1))
    } else {
        ((x - 1 - x0) + (x0 + 1) * p0, (x - x0) + (x0 + 1) * p0)
    };
    (fl as u32, fh as u32, ft as u32)
}

impl Coder for Ec<'_, '_> {
    const ENCODE: bool = false;

    fn tell_frac(&self) -> i32 {
        Ec::tell_frac(self)
    }

    fn code_bit_logp(&mut self, _val: bool, logp: u32) -> bool {
        self.bit_logp(logp)
    }

    fn code_uint(&mut self, _val: u32, ft: u32) -> u32 {
        self.uint(ft)
    }

    fn code_bits(&mut self, _val: u32, bits: u32) -> u32 {
        self.bits(bits)
    }

    fn code_theta_step(&mut self, _itheta: i32, qn: i32) -> i32 {
        let p0 = 3;
        let x0 = qn / 2;
        let ft = p0 * (x0 + 1) + x0;
        let fs = self.decode(ft as u32) as i32;
        let x = if fs < (x0 + 1) * p0 {
            fs / p0
        } else {
            x0 + 1 + (fs - (x0 + 1) * p0)
        };
        let (fl, fh, ft) = theta_step_symbol(x, qn);
        self.update(fl, fh, ft);
        x
    }

    fn code_theta_triangle(&mut self, _itheta: i32, qn: i32) -> i32 {
        let ft = ((qn >> 1) + 1) * ((qn >> 1) + 1);
        let fm = self.decode(ft as u32) as i32;
        let (itheta, fs, fl);
        if fm < (((qn >> 1) * ((qn >> 1) + 1)) >> 1) {
            itheta = (isqrt32(8 * fm as u32 + 1) as i32 - 1) >> 1;
            fs = itheta + 1;
            fl = (itheta * (itheta + 1)) >> 1;
        } else {
            itheta = (2 * (qn + 1) - isqrt32(8 * (ft - fm - 1) as u32 + 1) as i32) >> 1;
            fs = qn + 1 - itheta;
            fl = ft - (((qn + 1 - itheta) * (qn + 2 - itheta)) >> 1);
        }
        self.update(fl as u32, (fl + fs) as u32, ft as u32);
        itheta
    }

    fn code_pvq(&mut self, x: &mut [f32], k: usize, spread: i32, b: usize, gain: f32) -> u32 {
        alg_unquant(x, k, spread, b, self, gain)
    }
}

impl Coder for Enc<'_> {
    const ENCODE: bool = true;

    fn tell_frac(&self) -> i32 {
        Enc::tell_frac(self)
    }

    fn code_bit_logp(&mut self, val: bool, logp: u32) -> bool {
        self.bit_logp(val, logp);
        val
    }

    fn code_uint(&mut self, val: u32, ft: u32) -> u32 {
        self.uint(val, ft);
        val
    }

    fn code_bits(&mut self, val: u32, bits: u32) -> u32 {
        self.bits(val, bits);
        val
    }

    fn code_theta_step(&mut self, itheta: i32, qn: i32) -> i32 {
        let (fl, fh, ft) = theta_step_symbol(itheta, qn);
        self.encode(fl, fh, ft);
        itheta
    }

    fn code_theta_triangle(&mut self, itheta: i32, qn: i32) -> i32 {
        let half = qn >> 1;
        let ft = (half + 1) * (half + 1);
        let (fs, fl) = if itheta <= half {
            (itheta + 1, (itheta * (itheta + 1)) >> 1)
        } else {
            (
                qn + 1 - itheta,
                ft - (((qn + 1 - itheta) * (qn + 2 - itheta)) >> 1),
            )
        };
        self.encode(fl as u32, (fl + fs) as u32, ft as u32);
        itheta
    }

    fn code_pvq(&mut self, x: &mut [f32], k: usize, spread: i32, b: usize, gain: f32) -> u32 {
        alg_quant(x, k, spread, b, self, gain)
    }
}
