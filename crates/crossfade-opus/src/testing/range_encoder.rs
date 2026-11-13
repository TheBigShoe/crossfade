//! The range ENCODER of RFC 6716 section 5.1 (libopus `celt/entenc.c`), ported for tests: it
//! writes what the decoder must read back, symbol for symbol, with the same `tell`,
//! `tell_frac` and final range at every step. Not used by the decoder.

use crate::range::{
    CODE_BOT, CODE_SHIFT, CODE_TOP, SYM_BITS, SYM_MAX, WINDOW_SIZE, ilog, tell_frac,
};

const UINT_BITS: u32 = 8;

/// A range encoder writing into a buffer of fixed size, `storage` bytes: range-coded data from
/// the front, raw bits from the back, as the decoder reads them.
#[derive(Debug, Clone)]
pub struct RangeEncoder {
    buf: Vec<u8>,
    storage: usize,
    end_offs: usize,
    end_window: u32,
    nend_bits: u32,
    nbits_total: i32,
    offs: usize,
    rng: u32,
    val: u32,
    /// Bytes of 0xFF held back until a carry is known.
    ext: u32,
    /// The byte held back for a possible carry; `None` before the first.
    rem: Option<u32>,
    error: bool,
}

impl RangeEncoder {
    pub fn new(storage: usize) -> Self {
        Self {
            buf: vec![0; storage],
            storage,
            end_offs: 0,
            end_window: 0,
            nend_bits: 0,
            nbits_total: 33,
            offs: 0,
            rng: CODE_TOP,
            val: 0,
            ext: 0,
            rem: None,
            error: false,
        }
    }

    fn write_byte(&mut self, value: u32) {
        if self.offs + self.end_offs >= self.storage {
            self.error = true;
            return;
        }
        self.buf[self.offs] = value as u8;
        self.offs += 1;
    }

    fn write_byte_at_end(&mut self, value: u32) {
        if self.offs + self.end_offs >= self.storage {
            self.error = true;
            return;
        }
        self.end_offs += 1;
        self.buf[self.storage - self.end_offs] = value as u8;
    }

    fn carry_out(&mut self, c: u32) {
        if c != SYM_MAX {
            let carry = c >> SYM_BITS;
            if let Some(rem) = self.rem {
                self.write_byte(rem + carry);
            }
            if self.ext > 0 {
                let sym = (SYM_MAX + carry) & SYM_MAX;
                while self.ext > 0 {
                    self.write_byte(sym);
                    self.ext -= 1;
                }
            }
            self.rem = Some(c & SYM_MAX);
        } else {
            self.ext += 1;
        }
    }

    fn normalize(&mut self) {
        while self.rng <= CODE_BOT {
            self.carry_out(self.val >> CODE_SHIFT);
            self.val = (self.val << SYM_BITS) & (CODE_TOP - 1);
            self.rng <<= SYM_BITS;
            self.nbits_total += SYM_BITS as i32;
        }
    }

    /// Codes the symbol `[fl, fh)` of total `ft`.
    pub fn encode(&mut self, fl: u32, fh: u32, ft: u32) {
        let r = self.rng / ft;
        if fl > 0 {
            self.val += self.rng - r * (ft - fl);
            self.rng = r * (fh - fl);
        } else {
            self.rng -= r * (ft - fh);
        }
        self.normalize();
    }

    /// [`encode`](Self::encode) with `ft = 1 << bits`.
    pub fn encode_bin(&mut self, fl: u32, fh: u32, bits: u32) {
        let r = self.rng >> bits;
        if fl > 0 {
            self.val += self.rng - r * ((1 << bits) - fl);
            self.rng = r * (fh - fl);
        } else {
            self.rng -= r * ((1 << bits) - fh);
        }
        self.normalize();
    }

    /// A binary symbol whose "1" has probability `1 / (1 << logp)`.
    pub fn enc_bit_logp(&mut self, val: bool, logp: u32) {
        let mut r = self.rng;
        let l = self.val;
        let s = r >> logp;
        r -= s;
        if val {
            self.val = l + r;
        }
        self.rng = if val { s } else { r };
        self.normalize();
    }

    /// Symbol `s` of an inverse-CDF table of `ftb` bits.
    pub fn enc_icdf(&mut self, s: usize, icdf: &[u8], ftb: u32) {
        let icdf: Vec<u32> = icdf.iter().map(|&v| u32::from(v)).collect();
        self.icdf(s, &icdf, ftb);
    }

    /// [`enc_icdf`](Self::enc_icdf) with a 16-bit table.
    pub fn enc_icdf16(&mut self, s: usize, icdf: &[u16], ftb: u32) {
        let icdf: Vec<u32> = icdf.iter().map(|&v| u32::from(v)).collect();
        self.icdf(s, &icdf, ftb);
    }

    fn icdf(&mut self, s: usize, icdf: &[u32], ftb: u32) {
        let r = self.rng >> ftb;
        if s > 0 {
            self.val += self.rng - r * icdf[s - 1];
            self.rng = r * (icdf[s - 1] - icdf[s]);
        } else {
            self.rng -= r * icdf[s];
        }
        self.normalize();
    }

    /// `fl` uniformly in `0..ft` (`ft` at least 2).
    pub fn enc_uint(&mut self, fl: u32, ft: u32) {
        let ft = ft - 1;
        let mut ftb = ilog(ft);
        if ftb > UINT_BITS {
            ftb -= UINT_BITS;
            let t = (ft >> ftb) + 1;
            let f = fl >> ftb;
            self.encode(f, f + 1, t);
            self.enc_bits(fl & ((1 << ftb) - 1), ftb);
        } else {
            self.encode(fl, fl + 1, ft + 1);
        }
    }

    /// The low `bits` bits of `fl` as raw bits at the end of the frame (`bits` at most 25).
    pub fn enc_bits(&mut self, fl: u32, bits: u32) {
        let mut window = self.end_window;
        let mut used = self.nend_bits;
        if used + bits > WINDOW_SIZE {
            loop {
                self.write_byte_at_end(window & SYM_MAX);
                window >>= SYM_BITS;
                used -= SYM_BITS;
                if used < SYM_BITS {
                    break;
                }
            }
        }
        // A full window and zero bits: C shifts by 32 (undefined, and adds nothing).
        window |= fl.checked_shl(used).unwrap_or(0);
        used += bits;
        self.end_window = window;
        self.nend_bits = used;
        self.nbits_total += bits as i32;
    }

    pub fn tell(&self) -> i32 {
        self.nbits_total - ilog(self.rng) as i32
    }

    pub fn tell_frac(&self) -> u32 {
        tell_frac(self.nbits_total, self.rng)
    }

    pub fn rng(&self) -> u32 {
        self.rng
    }

    /// Whether the buffer was too small.
    pub fn error(&self) -> bool {
        self.error
    }

    /// Ends the stream with the fewest bits that decode as intended whatever follows, and
    /// returns the frame (`storage` bytes); `None` when it did not fit.
    pub fn done(mut self) -> Option<Vec<u8>> {
        let mut l = 32 - ilog(self.rng) as i32;
        let mut msk = (CODE_TOP - 1) >> l;
        let mut end = self.val.wrapping_add(msk) & !msk;
        if (end | msk) >= self.val.wrapping_add(self.rng) {
            l += 1;
            msk >>= 1;
            end = self.val.wrapping_add(msk) & !msk;
        }
        while l > 0 {
            self.carry_out(end >> CODE_SHIFT);
            end = (end << SYM_BITS) & (CODE_TOP - 1);
            l -= SYM_BITS as i32;
        }
        if self.rem.is_some() || self.ext > 0 {
            self.carry_out(0);
        }
        let mut window = self.end_window;
        let mut used = self.nend_bits;
        while used >= SYM_BITS {
            self.write_byte_at_end(window & SYM_MAX);
            window >>= SYM_BITS;
            used -= SYM_BITS;
        }
        if !self.error {
            let gap = self.offs..self.storage - self.end_offs;
            self.buf[gap].fill(0);
            if used > 0 {
                if self.end_offs >= self.storage {
                    self.error = true;
                } else {
                    let l = (-l) as u32;
                    if self.offs + self.end_offs >= self.storage && l < used {
                        window &= (1 << l) - 1;
                        self.error = true;
                    }
                    self.buf[self.storage - self.end_offs - 1] |= window as u8;
                }
            }
        }
        (!self.error).then_some(self.buf)
    }

    /// The bytes a frame needs: range-coded (those written and those held back for a carry),
    /// raw, and one shared byte for leftover raw bits.
    pub fn bytes_needed(&self) -> usize {
        // done() may write up to 4 more range bytes (and a carried one) and the raw window.
        self.offs + self.ext as usize + self.end_offs + 6 + self.nend_bits.div_ceil(8) as usize
    }
}

/// One operation of a test stream: what was coded and how, so that a decoder can be asked for
/// the same thing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Symbol `[fl, fh)` of `ft` (1 <= ft <= 65535).
    Symbol {
        fl: u32,
        fh: u32,
        ft: u32,
    },
    /// Symbol `[fl, fh)` of `1 << bits` (bits <= 15).
    Bin {
        fl: u32,
        fh: u32,
        bits: u32,
    },
    BitLogp {
        val: bool,
        logp: u32,
    },
    Icdf {
        s: usize,
        icdf: Vec<u8>,
        ftb: u32,
    },
    Icdf16 {
        s: usize,
        icdf: Vec<u16>,
        ftb: u32,
    },
    Uint {
        val: u32,
        ft: u32,
    },
    Bits {
        val: u32,
        bits: u32,
    },
}

impl RangeEncoder {
    pub fn apply(&mut self, op: &Op) {
        match op {
            Op::Symbol { fl, fh, ft } => self.encode(*fl, *fh, *ft),
            Op::Bin { fl, fh, bits } => self.encode_bin(*fl, *fh, *bits),
            Op::BitLogp { val, logp } => self.enc_bit_logp(*val, *logp),
            Op::Icdf { s, icdf, ftb } => self.enc_icdf(*s, icdf, *ftb),
            Op::Icdf16 { s, icdf, ftb } => self.enc_icdf16(*s, icdf, *ftb),
            Op::Uint { val, ft } => self.enc_uint(*val, *ft),
            Op::Bits { val, bits } => self.enc_bits(*val, *bits),
        }
    }
}

/// `(tell, tell_frac, rng)` after each operation, and the frame.
pub type Encoded = (Vec<(i32, u32, u32)>, Vec<u8>);

/// [`Encoded`]: in the smallest buffer the
/// operations fit in, so that the decoder also reads past its end.
pub fn encode_ops(ops: &[Op]) -> Encoded {
    let mut probe = RangeEncoder::new(1 << 20);
    for op in ops {
        probe.apply(op);
    }
    let mut best = encode_into(ops, probe.bytes_needed()).expect("the estimate is enough");
    while let Some(smaller) = best
        .1
        .len()
        .checked_sub(1)
        .and_then(|n| encode_into(ops, n))
    {
        best = smaller;
    }
    best
}

/// The operations coded into a frame of `storage` bytes; `None` when they do not fit.
pub fn encode_into(ops: &[Op], storage: usize) -> Option<Encoded> {
    let mut enc = RangeEncoder::new(storage);
    let mut steps = Vec::with_capacity(ops.len());
    for op in ops {
        enc.apply(op);
        steps.push((enc.tell(), enc.tell_frac(), enc.rng()));
    }
    enc.done().map(|frame| (steps, frame))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_ops_fits_long_runs_of_held_back_bytes() {
        // Improbable "1"s keep the value at the top of the range: every byte out is 0xFF,
        // held back for a carry, so `offs` alone undercounts the frame.
        for n in [8, 64, 400] {
            let ops = vec![
                Op::BitLogp {
                    val: true,
                    logp: 15
                };
                n
            ];
            let (_, frame) = encode_ops(&ops);
            let mut dec = crate::RangeDecoder::new(&frame);
            assert!((0..n).all(|_| dec.dec_bit_logp(15)), "{n} ops");
        }
    }
}
