//! MSB-first bit packing with a 64-bit accumulator, appending to a caller's `Vec`. Not
//! `crate::bits::BitWriter`: that one pushes per byte, and Rice coding runs this per sample.

pub(super) struct BitSink<'a> {
    out: &'a mut Vec<u8>,
    /// The low `n` bits are pending; anything above is stale.
    acc: u64,
    n: u32,
}

impl<'a> BitSink<'a> {
    pub(super) fn new(out: &'a mut Vec<u8>) -> Self {
        Self { out, acc: 0, n: 0 }
    }

    /// The low `bits` bits of `value`; `bits` up to 32.
    #[inline]
    pub(super) fn put(&mut self, value: u64, bits: u32) {
        debug_assert!(bits <= 32);
        self.acc = (self.acc << bits) | (value & ((1u64 << bits) - 1));
        self.n += bits;
        if self.n >= 32 {
            self.n -= 32;
            let word = (self.acc >> self.n) as u32;
            self.out.extend_from_slice(&word.to_be_bytes());
        }
    }

    /// Two's complement over `bits` bits, up to 64 (a 32-bit stream's side channel has 33).
    #[inline]
    pub(super) fn put_signed(&mut self, value: i64, bits: u32) {
        if bits > 32 {
            self.put((value >> 32) as u64, bits - 32);
            self.put(value as u64, 32);
        } else {
            self.put(value as u64, bits);
        }
    }

    /// Rice codes of `values` at parameter `k` (below 32). The accumulator lives in locals
    /// for the whole run: kept in the struct, it goes back to memory around every push.
    pub(super) fn put_rice(&mut self, values: &[u32], k: u32) {
        let (mut acc, mut n) = (self.acc, self.n);
        let mask = (1u32 << k) - 1;
        for &v in values {
            let q = v >> k;
            if q + 1 + k <= 32 {
                // The quotient's zeros, the stop bit and the low bits as one field.
                let bits = q + 1 + k;
                acc = (acc << bits) | u64::from((1u32 << k) | (v & mask));
                n += bits;
                if n >= 32 {
                    n -= 32;
                    self.out
                        .extend_from_slice(&((acc >> n) as u32).to_be_bytes());
                }
            } else {
                (self.acc, self.n) = (acc, n);
                self.put_zeros(q);
                self.put(u64::from((1u32 << k) | (v & mask)), k + 1);
                (acc, n) = (self.acc, self.n);
            }
        }
        (self.acc, self.n) = (acc, n);
    }

    pub(super) fn put_zeros(&mut self, mut count: u32) {
        while count > 32 {
            self.put(0, 32);
            count -= 32;
        }
        self.put(0, count);
    }

    /// Pads with zero bits to a byte boundary and flushes.
    pub(super) fn finish(mut self) {
        let pad = (8 - self.n % 8) % 8;
        self.acc <<= pad;
        self.n += pad;
        for i in (0..self.n / 8).rev() {
            self.out.push((self.acc >> (8 * i)) as u8);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::BitReader;

    #[test]
    fn matches_the_reference_bit_reader() {
        let fields: Vec<(i64, u32)> = vec![
            (-1, 1),
            (-1, 3),
            (0x1234_5678, 32),
            (-5, 7),
            (-(1 << 32), 33),
            ((1 << 32) - 1, 33),
            (0, 0),
            (0x3FFF_FFFF, 31),
            (-2, 33),
            (1, 2),
        ];
        let mut out = Vec::new();
        let mut w = BitSink::new(&mut out);
        for &(v, n) in &fields {
            w.put_signed(v, n);
        }
        w.put_zeros(70);
        w.put(1, 1);
        w.finish();
        let mut r = BitReader::new(&out);
        for &(v, n) in &fields {
            assert_eq!(r.read_signed(n).unwrap(), v, "{n} bits");
        }
        assert_eq!(r.read_unary().unwrap(), 70);
        assert!(r.remaining_bits() < 8, "padding is under a byte");
    }

    #[test]
    fn a_rice_run_writes_what_single_puts_write() {
        let raw: Vec<u32> = (0..300u32)
            .map(|i| i.wrapping_mul(2_654_435_761) >> (i % 31))
            .collect();
        for k in [0u32, 1, 5, 14, 20, 30] {
            // Quotients up to a few hundred: some codes fit one field, some need two.
            let values: Vec<u32> = raw.iter().map(|&v| v % (300 << k.min(23))).collect();
            let (mut fast, mut slow) = (vec![7u8], vec![7u8]);
            let mut a = BitSink::new(&mut fast);
            a.put(5, 3);
            a.put_rice(&values, k);
            a.finish();
            let mut b = BitSink::new(&mut slow);
            b.put(5, 3);
            for &v in &values {
                b.put_zeros(v >> k);
                b.put(u64::from((1u32 << k) | (v & ((1u32 << k) - 1))), k + 1);
            }
            b.finish();
            assert_eq!(fast, slow, "k {k}");
        }
    }

    #[test]
    fn finish_pads_each_residue() {
        for bits in 0..40 {
            let mut out = vec![0xAA];
            let mut w = BitSink::new(&mut out);
            w.put_zeros(bits);
            w.put(1, 1);
            w.finish();
            assert_eq!(out.len(), 1 + (bits as usize + 1).div_ceil(8));
            assert_eq!(out[0], 0xAA, "existing bytes are kept");
        }
    }
}
