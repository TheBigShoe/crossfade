//! The range decoder of RFC 6716 section 4.1, ported from libopus `celt/entdec.c` and
//! `celt/entcode.c`: one per frame, shared by the SILK and CELT layers in that order.
//!
//! Range-coded symbols are read from the front of the frame and raw bits from its end; past
//! either end the decoder reads zeros, as the RFC requires. Everything is integer arithmetic,
//! bit-exact with the reference: [`RangeDecoder::tell`] and [`RangeDecoder::tell_frac`] drive
//! CELT's bit allocation and [`RangeDecoder::rng`] after a frame is the final range the test
//! vectors store per packet.
//!
//! Hostile input cannot make it panic or loop: the frame's bytes are only ever indexed within
//! bounds. Arguments outside a method's documented range (a caller's bug, not the stream's) set
//! the error flag and return an unspecified value instead of panicking.

pub(crate) const SYM_BITS: u32 = 8;
pub(crate) const SYM_MAX: u32 = (1 << SYM_BITS) - 1;
pub(crate) const CODE_TOP: u32 = 1 << 31;
pub(crate) const CODE_BOT: u32 = CODE_TOP >> SYM_BITS;
pub(crate) const CODE_SHIFT: u32 = 32 - SYM_BITS - 1;
/// Bits of the first byte that initialisation uses (`EC_CODE_EXTRA`).
const CODE_EXTRA: u32 = (32 - 2) % SYM_BITS + 1;
pub(crate) const WINDOW_SIZE: u32 = 32;
const UINT_BITS: u32 = 8;
/// Most raw bits one call may read.
const MAX_RAW_BITS: u32 = 25;

/// Bits needed to hold `x` (0 for 0): RFC 6716's `ilog`, libopus's `EC_ILOG`.
pub(crate) fn ilog(x: u32) -> u32 {
    32 - x.leading_zeros()
}

/// `ec_tell_frac`: bits used so far in 1/8 bit, an upper bound. libopus's table form, equal to
/// RFC 6716 4.1.6.2's three squarings for every value (a test checks all of them).
pub(crate) fn tell_frac(nbits_total: i32, rng: u32) -> u32 {
    const CORRECTION: [u32; 8] = [35733, 38967, 42495, 46340, 50535, 55109, 60097, 65535];
    let nbits = (nbits_total as u32).wrapping_shl(3);
    let l = ilog(rng);
    // After renormalisation rng > 2**23, so l >= 24; the shift guards a caller's bad state.
    let r = if l >= 16 {
        rng >> (l - 16)
    } else {
        rng << (16 - l)
    };
    let mut b = (r >> 12).saturating_sub(8).min(7);
    b += u32::from(r > CORRECTION[b as usize]);
    nbits.wrapping_sub((l << 3) + b)
}

/// The range decoder over one frame (libopus `ec_dec`).
#[derive(Debug, Clone)]
pub struct RangeDecoder<'a> {
    buf: &'a [u8],
    /// The frame's length; [`shrink`](Self::shrink) lowers it so raw bits end earlier.
    storage: usize,
    /// Bytes read from the end for raw bits.
    end_offs: usize,
    end_window: u32,
    nend_bits: u32,
    /// Whole bits read, plus those buffered in `rng` and two spare (RFC 6716 4.1.6).
    nbits_total: i32,
    /// Bytes read from the front by the range coder.
    offs: usize,
    rng: u32,
    val: u32,
    /// `rng / ft` of the last [`decode`](Self::decode), for [`update`](Self::update).
    ext: u32,
    /// The byte last read, whose low bit the next renormalisation uses.
    rem: u32,
    error: bool,
}

impl<'a> RangeDecoder<'a> {
    /// A decoder over `buf`, the whole frame (RFC 6716 4.1.1).
    pub fn new(buf: &'a [u8]) -> Self {
        let mut dec = Self {
            buf,
            storage: buf.len(),
            end_offs: 0,
            end_window: 0,
            nend_bits: 0,
            nbits_total: (32 + 1 - ((32 - CODE_EXTRA) / SYM_BITS) * SYM_BITS) as i32,
            offs: 0,
            rng: 1 << CODE_EXTRA,
            val: 0,
            ext: 0,
            rem: 0,
            error: false,
        };
        dec.rem = dec.read_byte();
        dec.val = dec.rng - 1 - (dec.rem >> (SYM_BITS - CODE_EXTRA));
        dec.normalize();
        dec
    }

    fn read_byte(&mut self) -> u32 {
        if self.offs < self.storage {
            let b = self.buf[self.offs];
            self.offs += 1;
            u32::from(b)
        } else {
            0
        }
    }

    fn read_byte_from_end(&mut self) -> u32 {
        if self.end_offs < self.storage {
            self.end_offs += 1;
            u32::from(self.buf[self.storage - self.end_offs])
        } else {
            0
        }
    }

    /// RFC 6716 4.1.2.1: until `rng > 2**23`, shift in a byte made of the last byte's low bit
    /// and the next byte's top seven.
    fn normalize(&mut self) {
        if self.rng == 0 {
            // Only a caller's invalid update gets here; carry on with a valid state.
            self.error = true;
            self.rng = 1;
        }
        while self.rng <= CODE_BOT {
            self.nbits_total = self.nbits_total.wrapping_add(SYM_BITS as i32);
            self.rng <<= SYM_BITS;
            let mut sym = self.rem;
            self.rem = self.read_byte();
            sym = ((sym << SYM_BITS) | self.rem) >> (SYM_BITS - CODE_EXTRA);
            self.val = ((self.val << SYM_BITS).wrapping_add(SYM_MAX & !sym)) & (CODE_TOP - 1);
        }
    }

    /// `ec_decode`: the value `fs` in `0..ft` that locates the next symbol of total `ft`
    /// (1 to 65535); follow with [`update`](Self::update) for the symbol containing it.
    pub fn decode(&mut self, ft: u32) -> u32 {
        if ft == 0 || ft > u32::from(u16::MAX) {
            self.error = true;
            self.ext = 1;
            return 0;
        }
        self.ext = self.rng / ft;
        let s = self.val / self.ext;
        ft - s.saturating_add(1).min(ft)
    }

    /// `ec_decode_bin`: [`decode`](Self::decode) with `ft = 1 << bits` (`bits` at most 15).
    pub fn decode_bin(&mut self, bits: u32) -> u32 {
        if bits > 15 {
            self.error = true;
            self.ext = 1;
            return 0;
        }
        self.ext = self.rng >> bits;
        let s = self.val / self.ext;
        (1 << bits) - s.saturating_add(1).min(1 << bits)
    }

    /// `ec_dec_update`: consumes the symbol `[fl, fh)` of total `ft` that the last
    /// [`decode`](Self::decode) or [`decode_bin`](Self::decode_bin) located.
    pub fn update(&mut self, fl: u32, fh: u32, ft: u32) {
        let s = self.ext.wrapping_mul(ft.wrapping_sub(fh));
        self.val = self.val.wrapping_sub(s);
        self.rng = if fl > 0 {
            self.ext.wrapping_mul(fh.wrapping_sub(fl))
        } else {
            self.rng.wrapping_sub(s)
        };
        self.normalize();
    }

    /// `ec_dec_bit_logp`: a binary symbol whose "1" has probability `1 / (1 << logp)`.
    pub fn dec_bit_logp(&mut self, logp: u32) -> bool {
        let r = self.rng;
        let d = self.val;
        let Some(s) = r.checked_shr(logp) else {
            self.error = true;
            return false;
        };
        let ret = d < s;
        if !ret {
            self.val = d - s;
        }
        self.rng = if ret { s } else { r - s };
        self.normalize();
        ret
    }

    /// `ec_dec_icdf`: a symbol from an inverse-CDF table of `ftb` bits (at most 8), whose
    /// entries fall strictly and end with 0 (RFC 6716 4.1.3.3).
    pub fn dec_icdf(&mut self, icdf: &[u8], ftb: u32) -> usize {
        self.icdf(icdf.len(), |k| u32::from(icdf[k]), ftb)
    }

    /// `ec_dec_icdf16`: [`dec_icdf`](Self::dec_icdf) with a 16-bit table (`ftb` at most 15).
    pub fn dec_icdf16(&mut self, icdf: &[u16], ftb: u32) -> usize {
        self.icdf(icdf.len(), |k| u32::from(icdf[k]), ftb)
    }

    fn icdf(&mut self, len: usize, icdf: impl Fn(usize) -> u32, ftb: u32) -> usize {
        let mut s = self.rng;
        let d = self.val;
        let r = s.checked_shr(ftb).unwrap_or(0);
        let mut t;
        let mut k = 0;
        loop {
            t = s;
            // A table without its final 0 stops at its end, as if it had one.
            s = if k < len { r.wrapping_mul(icdf(k)) } else { 0 };
            if d >= s || k + 1 >= len {
                break;
            }
            k += 1;
        }
        if d < s {
            // Only a malformed table (rising entries or no final 0) ends here.
            self.error = true;
            s = 0;
        }
        self.val = d - s;
        self.rng = t.wrapping_sub(s);
        self.normalize();
        k
    }

    /// `ec_dec_uint`: an integer uniform in `0..ft` (`ft` at least 2; 0 and 1 give 0 and read
    /// nothing). Above 8 bits the low bits are raw bits; a value past `ft - 1` means a corrupt
    /// frame: it sets the error flag and gives `ft - 1` (RFC 6716 4.1.5).
    pub fn dec_uint(&mut self, ft: u32) -> u32 {
        if ft <= 1 {
            return 0;
        }
        let ft = ft - 1;
        let ftb = ilog(ft);
        if ftb > UINT_BITS {
            let ftb = ftb - UINT_BITS;
            let top = (ft >> ftb) + 1;
            let s = self.decode(top);
            self.update(s, s + 1, top);
            let t = (s << ftb) | self.dec_bits(ftb);
            if t <= ft {
                return t;
            }
            self.error = true;
            ft
        } else {
            let s = self.decode(ft + 1);
            self.update(s, s + 1, ft + 1);
            s
        }
    }

    /// `ec_dec_bits`: `bits` raw bits (at most 25) from the end of the frame, least significant
    /// first (RFC 6716 4.1.4).
    pub fn dec_bits(&mut self, bits: u32) -> u32 {
        if bits > MAX_RAW_BITS {
            self.error = true;
            return 0;
        }
        let mut window = self.end_window;
        let mut available = self.nend_bits;
        if available < bits {
            loop {
                window |= self.read_byte_from_end() << available;
                available += SYM_BITS;
                if available > WINDOW_SIZE - SYM_BITS {
                    break;
                }
            }
        }
        let ret = window & ((1 << bits) - 1);
        self.end_window = window >> bits;
        self.nend_bits = available - bits;
        self.nbits_total = self.nbits_total.wrapping_add(bits as i32);
        ret
    }

    /// `ec_tell`: whole bits used so far, rounded up (`ceil(tell_frac / 8)`).
    pub fn tell(&self) -> i32 {
        self.nbits_total.wrapping_sub(ilog(self.rng) as i32)
    }

    /// `ec_tell_frac`: bits used so far in 1/8 bit, a bit-exact upper bound.
    pub fn tell_frac(&self) -> u32 {
        tell_frac(self.nbits_total, self.rng)
    }

    /// The range: after the last symbol of a frame, the "final range" an encoder reports.
    pub fn rng(&self) -> u32 {
        self.rng
    }

    /// Whether a corrupt value (or an invalid argument) was met (`ec_get_error`).
    pub fn error(&self) -> bool {
        self.error
    }

    /// The frame's length in bytes, as the decoder sees it.
    pub fn storage(&self) -> usize {
        self.storage
    }

    /// Takes `bytes` off the end of the frame before any raw bits are read, as the top level
    /// does for a hybrid frame's CELT redundancy (`dec.storage -= redundancy_bytes`).
    pub fn shrink(&mut self, bytes: usize) {
        self.storage = self.storage.saturating_sub(bytes);
    }

    /// Bytes the range coder has read from the front (`ec_range_bytes`).
    pub fn range_bytes(&self) -> usize {
        self.offs
    }
}

#[cfg(test)]
#[path = "range_tests.rs"]
mod tests;
