//! Subframes (RFC 9639 section 9.2): the bit reader the decoder runs on, residual decoding and
//! prediction.
//!
//! Samples are `i32`, or `i64` for the side channel of a 32-bit stream (33 bits). Everything a
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
    fn read_rice<T: Sample>(&mut self, out: &mut [T], k: u32) -> Result<()> {
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
            *v = T::from_i64(((folded >> 1) as i64) ^ -((folded & 1) as i64));
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

/// The sample type a subframe is decoded into: `i32` when the stream's depth leaves room for
/// it (nearly always, and much faster), `i64` for the side channel of a 32-bit stream.
pub(super) trait Sample: Copy + Default {
    /// Truncating: a corrupt stream decodes to noise, the frame CRC catches it.
    fn from_i64(v: i64) -> Self;
    fn wadd(self, o: Self) -> Self;
    fn wsub(self, o: Self) -> Self;
    fn wmul(self, k: i32) -> Self;
    fn wshl(self, n: u32) -> Self;
    /// Adds the LPC prediction to the residuals in place. `coefs[k]` multiplies the sample
    /// `order - k` places back, i.e. the stream's order reversed so the inner loop runs forward
    /// over the history. `bps` is the width of a valid sample, which bounds the prediction sum.
    fn restore_lpc(out: &mut [Self], coefs: &[i32], shift: u32, bps: u32);
}

impl Sample for i64 {
    fn from_i64(v: i64) -> Self {
        v
    }
    fn wadd(self, o: Self) -> Self {
        self.wrapping_add(o)
    }
    fn wsub(self, o: Self) -> Self {
        self.wrapping_sub(o)
    }
    fn wmul(self, k: i32) -> Self {
        self.wrapping_mul(i64::from(k))
    }
    fn wshl(self, n: u32) -> Self {
        self.wrapping_shl(n)
    }
    fn restore_lpc(out: &mut [i64], coefs: &[i32], shift: u32, _bps: u32) {
        let order = coefs.len();
        for i in order..out.len() {
            let (head, tail) = out.split_at_mut(i);
            let sum = head[i - order..]
                .iter()
                .zip(coefs)
                .fold(0i64, |acc, (&h, &c)| {
                    acc.wrapping_add(h.wrapping_mul(i64::from(c)))
                });
            tail[0] = tail[0].wrapping_add(sum >> shift);
        }
    }
}

impl Sample for i32 {
    fn from_i64(v: i64) -> Self {
        v as i32
    }
    fn wadd(self, o: Self) -> Self {
        self.wrapping_add(o)
    }
    fn wsub(self, o: Self) -> Self {
        self.wrapping_sub(o)
    }
    fn wmul(self, k: i32) -> Self {
        self.wrapping_mul(k)
    }
    fn wshl(self, n: u32) -> Self {
        self.wrapping_shl(n)
    }
    fn restore_lpc(out: &mut [i32], coefs: &[i32], shift: u32, bps: u32) {
        // Valid history is below 2^(bps-1) in magnitude, so this bounds the sum of products: when
        // it fits 32 bits the products need no 64-bit accumulator.
        let bound = coefs
            .iter()
            .map(|c| u64::from(c.unsigned_abs()))
            .sum::<u64>()
            << (bps - 1);
        lpc_i32(out, coefs, shift, bound < 1 << 31);
    }
}

/// `narrow`: the sum of products wraps at 32 bits, which equals the exact sum when it fits.
fn lpc_i32(out: &mut [i32], coefs: &[i32], shift: u32, narrow: bool) {
    macro_rules! by_order {
            ($($r:literal $f:ident)*; $($n:literal)*) => {
                match (coefs.len(), narrow) {
                    $(($r, true) => $f::<false>(out, coefs, shift),
                    ($r, false) => $f::<true>(out, coefs, shift),)*
                    $(($n, true) => lpc32::<$n, false>(out, coefs, shift),
                    ($n, false) => lpc32::<$n, true>(out, coefs, shift),)*
                    _ => lpc_any(out, coefs, shift),
                }
            };
        }
    by_order!(
        1 lpc_r1 2 lpc_r2 3 lpc_r3 4 lpc_r4 5 lpc_r5 6 lpc_r6 7 lpc_r7 8 lpc_r8 9 lpc_r9
        10 lpc_r10 11 lpc_r11 12 lpc_r12;
        13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32
    );
}

/// `WIDE` picks the 64-bit accumulator; the order is a constant so the taps unroll.
fn lpc32<const N: usize, const WIDE: bool>(out: &mut [i32], coefs: &[i32], shift: u32) {
    let Ok(c) = <&[i32; N]>::try_from(coefs) else {
        return;
    };
    for i in N..out.len() {
        let h = <&[i32; N]>::try_from(&out[i - N..i]).unwrap();
        let mut sum = 0i64;
        for j in 0..N {
            sum = sum.wrapping_add(i64::from(h[j]) * i64::from(c[j]));
        }
        let p = if WIDE || sum == i64::from(sum as i32) {
            (sum >> shift) as i32
        } else {
            (sum as i32) >> shift
        };
        out[i] = out[i].wrapping_add(p);
    }
}

/// LPC restoration with the history in named locals, for the orders encoders actually use.
/// Reading the history back from `out` waits on the store just made (and mixed-width reloads of
/// it defeat store forwarding); in registers, with the newest sample added last, one multiply,
/// add, shift and add separate one sample from the next.
macro_rules! lpc_regs {
    ($name:ident, $n:literal: $first:ident $fi:literal $(, $rest:ident $ri:literal)*) => {
        fn $name<const WIDE: bool>(out: &mut [i32], coefs: &[i32], shift: u32) {
            let Ok(c) = <&[i32; $n]>::try_from(coefs) else {
                return;
            };
            let Some((&mut [mut $first $(, mut $rest)*], tail)) =
                out.split_first_chunk_mut::<$n>()
            else {
                return;
            };
            for v in tail {
                // Always 64-bit multiply-adds: NEON has no 64-bit lane multiply, so this stays
                // scalar, where 32-bit products get vectorised around the loop-carried history.
                // The low 32 bits are the narrow sum, wrapped exactly as before.
                let sum = (i64::from($first) * i64::from(c[$fi]))
                    $(.wrapping_add(i64::from($rest) * i64::from(c[$ri])))*;
                let p = if WIDE || sum == i64::from(sum as i32) {
                    (sum >> shift) as i32
                } else {
                    (sum as i32) >> shift
                };
                let x = v.wrapping_add(p);
                *v = x;
                ($first $(, $rest)*) = ($($rest, )* x);
            }
        }
    };
}

lpc_regs!(lpc_r1, 1: a 0);
lpc_regs!(lpc_r2, 2: a 0, b 1);
lpc_regs!(lpc_r3, 3: a 0, b 1, d 2);
lpc_regs!(lpc_r4, 4: a 0, b 1, d 2, e 3);
lpc_regs!(lpc_r5, 5: a 0, b 1, d 2, e 3, f 4);
lpc_regs!(lpc_r6, 6: a 0, b 1, d 2, e 3, f 4, g 5);
lpc_regs!(lpc_r7, 7: a 0, b 1, d 2, e 3, f 4, g 5, h 6);
lpc_regs!(lpc_r8, 8: a 0, b 1, d 2, e 3, f 4, g 5, h 6, i 7);
lpc_regs!(lpc_r9, 9: a 0, b 1, d 2, e 3, f 4, g 5, h 6, i 7, j 8);
lpc_regs!(lpc_r10, 10: a 0, b 1, d 2, e 3, f 4, g 5, h 6, i 7, j 8, k 9);
lpc_regs!(lpc_r11, 11: a 0, b 1, d 2, e 3, f 4, g 5, h 6, i 7, j 8, k 9, l 10);
lpc_regs!(lpc_r12, 12: a 0, b 1, d 2, e 3, f 4, g 5, h 6, i 7, j 8, k 9, l 10, m 11);

/// An order the table above does not list cannot occur (1 to 32); this keeps the match total.
fn lpc_any(out: &mut [i32], coefs: &[i32], shift: u32) {
    let order = coefs.len();
    for i in order..out.len() {
        let sum = out[i - order..i]
            .iter()
            .zip(coefs)
            .fold(0i64, |acc, (&h, &c)| {
                acc.wrapping_add(i64::from(h) * i64::from(c))
            });
        out[i] = out[i].wrapping_add((sum >> shift) as i32);
    }
}

/// Fills `out[order..]` from the coded residual section.
fn read_residual<T: Sample>(r: &mut Bits, out: &mut [T], order: usize) -> Result<()> {
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
                *v = T::from_i64(r.read_signed(bits)?);
            }
        } else {
            r.read_rice(part, param as u32)?;
        }
    }
    Ok(())
}

/// Adds the fixed predictor of `order` (0 to 4) to the residuals in place. The history lives in
/// locals so the loop carries it in registers.
fn restore_fixed<T: Sample>(out: &mut [T], order: usize) {
    match order {
        1 if !out.is_empty() => {
            let mut a = out[0];
            for v in &mut out[1..] {
                a = v.wadd(a);
                *v = a;
            }
        }
        2 if out.len() >= 2 => {
            let (mut b, mut a) = (out[0], out[1]);
            for v in &mut out[2..] {
                let x = v.wadd(a.wmul(2).wsub(b));
                (b, a) = (a, x);
                *v = x;
            }
        }
        3 if out.len() >= 3 => {
            let (mut c, mut b, mut a) = (out[0], out[1], out[2]);
            for v in &mut out[3..] {
                let x = v.wadd(a.wmul(3).wsub(b.wmul(3)).wadd(c));
                (c, b, a) = (b, a, x);
                *v = x;
            }
        }
        4 if out.len() >= 4 => {
            let (mut d, mut c, mut b, mut a) = (out[0], out[1], out[2], out[3]);
            for v in &mut out[4..] {
                let x = v.wadd(a.wmul(4).wsub(b.wmul(6)).wadd(c.wmul(4)).wsub(d));
                (d, c, b, a) = (c, b, a, x);
                *v = x;
            }
        }
        _ => {}
    }
}

/// Decodes one subframe of `bps` bits per sample (already widened for a side channel) into
/// `out`, which becomes `block` samples long. `bps` must fit `T`.
pub(super) fn read_subframe<T: Sample>(
    r: &mut Bits,
    out: &mut Vec<T>,
    block: usize,
    bps: u32,
) -> Result<()> {
    // Every path below writes all `block` samples before it succeeds, so stale contents of a
    // reused buffer never show.
    out.resize(block, T::default());
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
            let v = T::from_i64(r.read_signed(bps)?);
            out.fill(v);
        }
        1 => {
            for v in out.iter_mut() {
                *v = T::from_i64(r.read_signed(bps)?);
            }
        }
        8..=12 => {
            let order = (kind - 8) as usize;
            if order > block {
                return Err(malformed("predictor order exceeds the block size"));
            }
            for v in &mut out[..order] {
                *v = T::from_i64(r.read_signed(bps)?);
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
                *v = T::from_i64(r.read_signed(bps)?);
            }
            let precision = r.read(4)? as u32 + 1;
            if precision == 16 {
                return Err(malformed("forbidden coefficient precision"));
            }
            let shift = r.read_signed(5)?;
            let Ok(shift) = u32::try_from(shift) else {
                return Err(malformed("negative prediction shift"));
            };
            let mut coefs = [0i32; 32];
            for c in &mut coefs[..order] {
                *c = r.read_signed(precision)? as i32;
            }
            let coefs = &mut coefs[..order];
            coefs.reverse();
            read_residual(r, out, order)?;
            T::restore_lpc(out, coefs, shift, bps);
        }
        _ => return Err(malformed("reserved subframe type")),
    }
    if wasted > 0 {
        for v in out.iter_mut() {
            *v = v.wshl(wasted);
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

    /// `bps`-bit values, with the extremes over-represented.
    fn sample(bps: u32) -> impl Strategy<Value = i64> {
        let (lo, hi) = (-(1i64 << (bps - 1)), (1i64 << (bps - 1)) - 1);
        prop_oneof![Just(lo), Just(hi), Just(0), lo..=hi]
    }

    /// A valid stream's samples, their LPC residuals and the coefficients, in the order
    /// `restore_lpc` takes them (reversed). `None` when a residual would not fit 32 bits.
    fn lpc_case() -> impl Strategy<Value = (u32, u32, Vec<i32>, Vec<i64>)> {
        (4u32..=32, 1u32..=15, 1usize..=32, 0u32..=15).prop_flat_map(
            |(bps, precision, order, shift)| {
                let c = (-(1i32 << (precision - 1)), (1i32 << (precision - 1)) - 1);
                (
                    Just(bps),
                    Just(shift),
                    prop::collection::vec(prop_oneof![Just(c.0), Just(c.1), c.0..=c.1], order),
                    prop::collection::vec(sample(bps), order..order + 40),
                )
            },
        )
    }

    /// The plain loop the kernels must equal on any input, valid or not: a 32-bit wrapping sum
    /// when `narrow`, a 64-bit one otherwise.
    fn lpc_reference(out: &mut [i32], coefs: &[i32], shift: u32, narrow: bool) {
        let order = coefs.len();
        for i in order..out.len() {
            let h = &out[i - order..i];
            let p = if narrow {
                h.iter()
                    .zip(coefs)
                    .fold(0i32, |a, (&h, &c)| a.wrapping_add(h.wrapping_mul(c)))
                    >> shift
            } else {
                (h.iter().zip(coefs).fold(0i64, |a, (&h, &c)| {
                    a.wrapping_add(i64::from(h) * i64::from(c))
                }) >> shift) as i32
            };
            out[i] = out[i].wrapping_add(p);
        }
    }

    proptest! {
        // Corrupt data that got past the CRC must decode to the same noise on every path.
        #[test]
        fn every_lpc_kernel_equals_the_plain_loop_on_any_input(
            coefs in prop::collection::vec(any::<i32>(), 1..=32),
            data in prop::collection::vec(prop_oneof![Just(i32::MIN), Just(i32::MAX), any::<i32>()], 0..80),
            shift in 0u32..=15,
            narrow in any::<bool>(),
        ) {
            let mut fast = data.clone();
            lpc_i32(&mut fast, &coefs, shift, narrow);
            let mut slow = data;
            lpc_reference(&mut slow, &coefs, shift, narrow);
            prop_assert_eq!(fast, slow);
        }

        // Whatever predictor the fast path takes (32-bit sums, 64-bit sums of 32-bit
        // products, or the i64 samples), a valid stream comes back exactly.
        #[test]
        fn lpc_restore_inverts_prediction_on_every_path((bps, shift, coefs, samples) in lpc_case()) {
            let order = coefs.len();
            let mut residual = samples.clone();
            for i in order..samples.len() {
                let sum: i64 = (0..order).map(|j| i64::from(coefs[j]) * samples[i - order + j]).sum();
                residual[i] = samples[i] - (sum >> shift);
            }
            prop_assume!(residual.iter().all(|&v| i32::try_from(v).is_ok()));
            let mut narrow: Vec<i32> = residual.iter().map(|&v| v as i32).collect();
            <i32 as Sample>::restore_lpc(&mut narrow, &coefs, shift, bps);
            prop_assert_eq!(narrow.iter().map(|&v| i64::from(v)).collect::<Vec<_>>(), samples.clone());
            let mut wide = residual;
            <i64 as Sample>::restore_lpc(&mut wide, &coefs, shift, bps);
            prop_assert_eq!(wide, samples);
        }

        #[test]
        fn fixed_restore_is_the_same_in_both_widths(
            order in 0usize..=4,
            data in prop::collection::vec(sample(32), 0..30),
        ) {
            let mut narrow: Vec<i32> = data.iter().map(|&v| v as i32).collect();
            let mut wide = data;
            restore_fixed(&mut narrow, order);
            restore_fixed(&mut wide, order);
            // Wrapping arithmetic agrees on the low 32 bits.
            prop_assert_eq!(narrow, wide.iter().map(|&v| v as i32).collect::<Vec<_>>());
        }

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
