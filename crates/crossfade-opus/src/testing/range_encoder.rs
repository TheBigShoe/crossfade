//! Test streams for the range coder: operations with what was coded and how, so that a decoder
//! can be asked for the same thing. The encoder itself is [`crate::range_enc`].

pub use crate::range_enc::RangeEncoder;

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
