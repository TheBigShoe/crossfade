//! MSB-first bit reading. Reading past the end yields zero bits instead of failing, so hot
//! loops carry no error paths; callers check [`BitReader::overrun`] once per syntactic
//! element. Every loop in the syntax is bounded by counts, so zeros cannot make one endless.
//!
//! The next bits wait left-aligned in a 64-bit word, refilled eight bytes at a time, so a
//! read is a shift and a refill is one load for every seven bytes.

#[derive(Clone, Copy)]
pub(crate) struct BitReader<'a> {
    data: &'a [u8],
    /// The next byte to load into `cache`; may run past the end, where bytes read as zero.
    byte: usize,
    /// Unread bits, left-aligned; the bits below the first `count` are zero.
    cache: u64,
    count: u32,
}

impl<'a> BitReader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte: 0,
            cache: 0,
            count: 0,
        }
    }

    /// Tops `cache` up to at least 56 bits.
    #[inline(always)]
    fn refill(&mut self) {
        let take = (63 - self.count) >> 3;
        let word = match self.data.get(self.byte..self.byte.wrapping_add(8)) {
            Some(b) => u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
            None => self.tail_word(),
        };
        // Only the whole bytes `take` counts are kept, so the bits below stay zero.
        let kept = word & !(u64::MAX >> (take * 8));
        self.cache |= kept >> self.count;
        self.byte = self.byte.saturating_add(take as usize);
        self.count += take * 8;
    }

    #[cold]
    fn tail_word(&self) -> u64 {
        let mut b = [0u8; 8];
        if let Some(rest) = self.data.get(self.byte..) {
            b[..rest.len()].copy_from_slice(rest);
        }
        u64::from_be_bytes(b)
    }

    /// The next 32 bits, left-aligned, zero past the end.
    #[inline(always)]
    pub(crate) fn peek32(&mut self) -> u32 {
        if self.count < 32 {
            self.refill();
        }
        (self.cache >> 32) as u32
    }

    /// Drops `n` bits a [`peek32`](Self::peek32) has just shown (at most 32).
    #[inline(always)]
    pub(crate) fn consume(&mut self, n: u32) {
        debug_assert!(n <= self.count);
        self.cache <<= n;
        self.count -= n;
    }

    /// `n` bits (0 to 32) as an unsigned number.
    #[inline(always)]
    pub(crate) fn read(&mut self, n: u32) -> u32 {
        debug_assert!(n <= 32);
        if n == 0 {
            return 0;
        }
        let v = self.peek32() >> (32 - n);
        self.consume(n);
        v
    }

    #[inline(always)]
    pub(crate) fn bit(&mut self) -> bool {
        self.read(1) != 0
    }

    /// The bit position: bits consumed so far.
    #[inline]
    fn pos(&self) -> usize {
        self.byte
            .saturating_mul(8)
            .saturating_sub(self.count as usize)
    }

    /// Moves to bit `pos`, emptying the cache.
    fn seek(&mut self, pos: usize) {
        self.byte = pos >> 3;
        self.cache = 0;
        self.count = 0;
        let rest = (pos & 7) as u32;
        if rest > 0 {
            self.refill();
            self.consume(rest);
        }
    }

    #[inline]
    pub(crate) fn skip(&mut self, n: usize) {
        if n <= self.count as usize {
            self.consume(n as u32);
        } else {
            self.seek(self.pos().saturating_add(n));
        }
    }

    /// The byte the next bit is in.
    #[cfg(feature = "decoder")]
    pub(crate) fn byte_pos(&self) -> usize {
        self.pos() / 8
    }

    pub(crate) fn len_bits(&self) -> usize {
        self.data.len() * 8
    }

    /// Bits left before the end; 0 once past it.
    pub(crate) fn remaining(&self) -> usize {
        self.len_bits().saturating_sub(self.pos())
    }

    /// Whether more bits were consumed than the input has.
    pub(crate) fn overrun(&self) -> bool {
        self.pos() > self.len_bits()
    }

    /// Skips to the next byte boundary, counted from the start of the input.
    pub(crate) fn byte_align(&mut self) {
        let pos = self.pos();
        self.skip(((pos + 7) & !7) - pos);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_msb_first_across_bytes_and_zeros_past_the_end() {
        let mut r = BitReader::new(&[0b1010_0000, 0xFF, 0x01]);
        assert_eq!(r.read(3), 0b101);
        assert_eq!(r.read(9), 0b0_0000_1111);
        assert_eq!(r.read(12), 0xF01);
        assert!(!r.overrun());
        assert_eq!(r.read(32), 0);
        assert!(r.overrun());
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn aligns_to_bytes() {
        let mut r = BitReader::new(&[0xFF, 0x80]);
        r.read(1);
        r.byte_align();
        assert_eq!(r.remaining(), 8);
        assert!(r.bit());
        r.byte_align();
        r.byte_align();
        assert_eq!(r.remaining(), 0);
    }

    /// The bit at `pos` of `data`, zero past the end.
    fn bit_at(data: &[u8], pos: usize) -> u32 {
        data.get(pos / 8)
            .map_or(0, |b| u32::from(b >> (7 - pos % 8)) & 1)
    }

    proptest::proptest! {
        /// Any mix of reads, skips and alignments sees the bits a one-bit-at-a-time reader
        /// sees, and agrees on the position.
        #[test]
        fn matches_a_bit_by_bit_reader(
            data in proptest::collection::vec(proptest::num::u8::ANY, 0..40),
            ops in proptest::collection::vec((0u8..4, 0u32..33), 0..60),
        ) {
            let mut r = BitReader::new(&data);
            let mut pos = 0usize;
            for (op, n) in ops {
                match op {
                    0 => {
                        let want = (0..n as usize)
                            .fold(0u64, |v, i| v << 1 | u64::from(bit_at(&data, pos + i)));
                        proptest::prop_assert_eq!(u64::from(r.read(n)), want);
                        pos += n as usize;
                    }
                    1 => {
                        let n = n as usize * 7;
                        r.skip(n);
                        pos += n;
                    }
                    2 => {
                        r.byte_align();
                        pos = (pos + 7) & !7;
                    }
                    _ => {
                        let want = (0..32).fold(0u32, |v, i| v << 1 | bit_at(&data, pos + i));
                        proptest::prop_assert_eq!(r.peek32(), want);
                    }
                }
                proptest::prop_assert_eq!(r.remaining(), (data.len() * 8).saturating_sub(pos));
                proptest::prop_assert_eq!(r.overrun(), pos > data.len() * 8);
            }
        }
    }
}
