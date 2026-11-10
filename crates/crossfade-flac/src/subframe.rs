//! Subframes (RFC 9639 section 9.2): the bit reader the decoder runs on, residual decoding and
//! prediction.
//!
//! Samples are `i64` because the side channel of a 32-bit stream needs 33 bits. Everything a
//! hostile stream controls goes through wrapping arithmetic, so corrupt data decodes to noise
//! (the frame CRC catches that) instead of panicking.

use crate::error::{Error, Result};

fn malformed(what: &str) -> Error {
    Error::Malformed(format!("flac: {what}"))
}

fn ran_out() -> Error {
    malformed("frame data ends inside a subframe")
}

/// MSB-first reader over one frame body. Not `crate::bits::BitReader`: the residual loop is
/// the decoder's hot path and wants a 64-bit window refilled a word at a time and a leading-zero
/// count for unary codes.
pub(super) struct Bits<'a> {
    data: &'a [u8],
    next: usize,
    /// The first `n` bits are the unread bits, MSB first. Below them sit either zeros or the
    /// stream bits that follow, which a refill then ORs in again unchanged.
    acc: u64,
    /// Never above 63, so every shift by an amount `<= n` stays in range.
    n: u32,
}

impl<'a> Bits<'a> {
    pub(super) fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            next: 0,
            acc: 0,
            n: 0,
        }
    }

    /// Brings `n` to at least 56 unless the data ends first.
    #[inline(always)]
    fn refill(&mut self) {
        if let Some(word) = self.data[self.next..].first_chunk::<8>() {
            self.acc |= u64::from_be_bytes(*word) >> self.n;
            let take = (63 - self.n) >> 3;
            self.next += take as usize;
            self.n += take * 8;
            return;
        }
        while self.n <= 55 && self.next < self.data.len() {
            self.acc |= u64::from(self.data[self.next]) << (56 - self.n);
            self.next += 1;
            self.n += 8;
        }
    }

    /// `k` up to 56 bits.
    pub(super) fn read(&mut self, k: u32) -> Result<u64> {
        debug_assert!(k <= 56);
        if k == 0 {
            return Ok(0);
        }
        if self.n < k {
            self.refill();
            if self.n < k {
                return Err(ran_out());
            }
        }
        let v = self.acc >> (64 - k);
        self.acc <<= k;
        self.n -= k;
        Ok(v)
    }

    /// Two's complement over `k` bits.
    pub(super) fn read_signed(&mut self, k: u32) -> Result<i64> {
        if k == 0 {
            return Ok(0);
        }
        let v = self.read(k)?;
        Ok(((v << (64 - k)) as i64) >> (64 - k))
    }

    /// Zero bits before the next one bit, which is consumed too.
    fn read_unary(&mut self) -> Result<u32> {
        let mut zeros = 0u32;
        loop {
            if self.n == 0 {
                self.refill();
                if self.n == 0 {
                    return Err(ran_out());
                }
            }
            let lead = self.acc.leading_zeros();
            if lead < self.n {
                self.acc <<= lead + 1;
                self.n -= lead + 1;
                return Ok(zeros.saturating_add(lead));
            }
            zeros = zeros.saturating_add(self.n);
            self.acc <<= self.n;
            self.n = 0;
        }
    }

    /// Rice-decodes `out.len()` residuals with parameter `k` (below 32). A code that fits the
    /// window is taken in one step; a long one, or one at the end of the data, goes the slow way.
    fn read_rice(&mut self, out: &mut [i64], k: u32) -> Result<()> {
        for v in out {
            if self.n < 48 {
                self.refill();
            }
            let lead = self.acc.leading_zeros();
            let used = lead + 1 + k;
            let folded = if used <= self.n {
                let low = ((self.acc << (lead + 1)) >> 1) >> (63 - k);
                self.acc <<= used;
                self.n -= used;
                (u64::from(lead) << k) | low
            } else {
                let q = u64::from(self.read_unary()?);
                (q << k) | self.read(k)?
            };
            *v = ((folded >> 1) as i64) ^ -((folded & 1) as i64);
        }
        Ok(())
    }

    /// Drops the bits up to the next byte boundary.
    pub(super) fn align(&mut self) {
        let drop = self.n % 8;
        self.acc <<= drop;
        self.n -= drop;
    }

    pub(super) fn bits_left(&self) -> u64 {
        u64::from(self.n) + 8 * (self.data.len() - self.next) as u64
    }
}

/// Fills `out[order..]` from the coded residual section.
fn read_residual(r: &mut Bits, out: &mut [i64], order: usize) -> Result<()> {
    let block = out.len();
    let (param_bits, escape) = match r.read(2)? {
        0 => (4, 15),
        1 => (5, 31),
        _ => return Err(malformed("reserved residual coding method")),
    };
    let partition_order = r.read(4)? as u32;
    let parts = 1usize << partition_order;
    if !block.is_multiple_of(parts) {
        return Err(malformed("partition order does not divide the block size"));
    }
    let size = block >> partition_order;
    // The RFC wants the partition strictly larger than the order; libFLAC, whose output is
    // the reference, accepts equal (an empty first partition), and so do we.
    if size < order {
        return Err(malformed(
            "residual partition smaller than the predictor order",
        ));
    }
    let mut at = order;
    for p in 0..parts {
        let count = if p == 0 { size - order } else { size };
        let part = &mut out[at..at + count];
        at += count;
        let param = r.read(param_bits)?;
        if param == escape {
            let bits = r.read(5)? as u32;
            for v in part {
                *v = r.read_signed(bits)?;
            }
        } else {
            r.read_rice(part, param as u32)?;
        }
    }
    Ok(())
}

/// Adds the fixed predictor of `order` (0 to 4) to the residuals in place.
fn restore_fixed(out: &mut [i64], order: usize) {
    let n = out.len();
    match order {
        1 => {
            for i in 1..n {
                out[i] = out[i].wrapping_add(out[i - 1]);
            }
        }
        2 => {
            for i in 2..n {
                let p = out[i - 1].wrapping_mul(2).wrapping_sub(out[i - 2]);
                out[i] = out[i].wrapping_add(p);
            }
        }
        3 => {
            for i in 3..n {
                let p = out[i - 1]
                    .wrapping_mul(3)
                    .wrapping_sub(out[i - 2].wrapping_mul(3))
                    .wrapping_add(out[i - 3]);
                out[i] = out[i].wrapping_add(p);
            }
        }
        4 => {
            for i in 4..n {
                let p = out[i - 1]
                    .wrapping_mul(4)
                    .wrapping_sub(out[i - 2].wrapping_mul(6))
                    .wrapping_add(out[i - 3].wrapping_mul(4))
                    .wrapping_sub(out[i - 4]);
                out[i] = out[i].wrapping_add(p);
            }
        }
        _ => {}
    }
}

/// `coefs[k]` multiplies the sample `order - k` places back, i.e. the stream's order reversed so
/// the inner loop runs forward over the history.
fn restore_lpc(out: &mut [i64], coefs: &[i64], shift: u32) {
    let order = coefs.len();
    for i in order..out.len() {
        let (head, tail) = out.split_at_mut(i);
        let sum = head[i - order..]
            .iter()
            .zip(coefs)
            .fold(0i64, |acc, (&h, &c)| acc.wrapping_add(h.wrapping_mul(c)));
        tail[0] = tail[0].wrapping_add(sum >> shift);
    }
}

/// Decodes one subframe of `bps` bits per sample (already widened for a side channel) into
/// `out`, which becomes `block` samples long.
pub(super) fn read_subframe(
    r: &mut Bits,
    out: &mut Vec<i64>,
    block: usize,
    bps: u32,
) -> Result<()> {
    out.clear();
    out.resize(block, 0);
    if r.read(1)? != 0 {
        return Err(malformed("subframe padding bit is set"));
    }
    let kind = r.read(6)? as u32;
    let wasted = if r.read(1)? == 1 {
        r.read_unary()?.saturating_add(1)
    } else {
        0
    };
    if wasted >= bps {
        return Err(malformed("wasted bits cover the whole sample"));
    }
    let bps = bps - wasted;
    match kind {
        0 => {
            let v = r.read_signed(bps)?;
            out.fill(v);
        }
        1 => {
            for v in out.iter_mut() {
                *v = r.read_signed(bps)?;
            }
        }
        8..=12 => {
            let order = (kind - 8) as usize;
            if order > block {
                return Err(malformed("predictor order exceeds the block size"));
            }
            for v in &mut out[..order] {
                *v = r.read_signed(bps)?;
            }
            read_residual(r, out, order)?;
            restore_fixed(out, order);
        }
        32..=63 => {
            let order = (kind - 31) as usize;
            if order > block {
                return Err(malformed("predictor order exceeds the block size"));
            }
            for v in &mut out[..order] {
                *v = r.read_signed(bps)?;
            }
            let precision = r.read(4)? as u32 + 1;
            if precision == 16 {
                return Err(malformed("forbidden coefficient precision"));
            }
            let shift = r.read_signed(5)?;
            let Ok(shift) = u32::try_from(shift) else {
                return Err(malformed("negative prediction shift"));
            };
            let mut coefs = [0i64; 32];
            for c in &mut coefs[..order] {
                *c = r.read_signed(precision)?;
            }
            let coefs = &mut coefs[..order];
            coefs.reverse();
            read_residual(r, out, order)?;
            restore_lpc(out, coefs, shift);
        }
        _ => return Err(malformed("reserved subframe type")),
    }
    if wasted > 0 {
        for v in out.iter_mut() {
            *v = v.wrapping_shl(wasted);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// One bit at a time, the obvious way.
    struct Slow<'a> {
        data: &'a [u8],
        pos: usize,
    }

    impl Slow<'_> {
        fn bit(&mut self) -> Option<u64> {
            let b = *self.data.get(self.pos / 8)?;
            let bit = (b >> (7 - self.pos % 8)) & 1;
            self.pos += 1;
            Some(u64::from(bit))
        }

        fn read(&mut self, k: u32) -> Option<u64> {
            let mut v = 0;
            for _ in 0..k {
                v = (v << 1) | self.bit()?;
            }
            Some(v)
        }

        fn unary(&mut self) -> Option<u32> {
            let mut zeros = 0u32;
            while self.bit()? == 0 {
                zeros += 1;
            }
            Some(zeros)
        }
    }

    #[derive(Debug, Clone)]
    enum Op {
        Read(u32),
        Unary,
        Rice(u32, usize),
        Align,
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            (0u32..=56).prop_map(Op::Read),
            Just(Op::Unary),
            (0u32..=31, 0usize..8).prop_map(|(k, n)| Op::Rice(k, n)),
            Just(Op::Align),
        ]
    }

    proptest! {
        // The windowed reader must agree with a bit-by-bit one on any mix of operations,
        // including the end of the data, where both must fail together.
        #[test]
        fn the_windowed_reader_equals_a_bitwise_one(
            data in prop::collection::vec(prop_oneof![Just(0u8), Just(0xff), any::<u8>()], 0..40),
            ops in prop::collection::vec(op(), 0..40),
        ) {
            let mut fast = Bits::new(&data);
            let mut slow = Slow { data: &data, pos: 0 };
            for op in ops {
                match op {
                    Op::Read(k) => {
                        let (f, s) = (fast.read(k).ok(), slow.read(k));
                        prop_assert_eq!(f, s);
                        if f.is_none() {
                            return Ok(());
                        }
                    }
                    Op::Unary => {
                        let (f, s) = (fast.read_unary().ok(), slow.unary());
                        prop_assert_eq!(f, s);
                        if f.is_none() {
                            return Ok(());
                        }
                    }
                    Op::Rice(k, count) => {
                        let mut got = vec![0i64; count];
                        let ok = fast.read_rice(&mut got, k).is_ok();
                        let mut want = Vec::new();
                        for _ in 0..count {
                            let Some(q) = slow.unary() else { break };
                            let Some(low) = slow.read(k) else { break };
                            let folded = (u64::from(q) << k) | low;
                            want.push(((folded >> 1) as i64) ^ -((folded & 1) as i64));
                        }
                        prop_assert_eq!(ok, want.len() == count);
                        if ok {
                            prop_assert_eq!(got, want);
                        } else {
                            // Both gave up: what is left is unspecified, so stop here.
                            return Ok(());
                        }
                    }
                    Op::Align => {
                        fast.align();
                        slow.pos = slow.pos.next_multiple_of(8);
                    }
                }
                prop_assert_eq!(
                    fast.bits_left(),
                    (data.len() * 8 - slow.pos.min(data.len() * 8)) as u64
                );
            }
        }
    }
}
