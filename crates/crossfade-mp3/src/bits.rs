//! An MSB-first bit reader with a 64-bit look-ahead. Bytes past the end of the data read as
//! zero, so a corrupt length can make a reader run on but never out of bounds; the decoder
//! checks positions against its limits instead.

pub(crate) struct Bits<'a> {
    data: &'a [u8],
    /// Bits consumed from the start of `data`.
    pos: usize,
    /// The 64 bits from `pos` on, left-aligned; valid for at least 57 bits after `refill`.
    cache: u64,
}

impl<'a> Bits<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        let mut b = Self {
            data,
            pos: 0,
            cache: 0,
        };
        b.refill();
        b
    }

    pub(crate) fn at(data: &'a [u8], pos: usize) -> Self {
        let mut b = Self::new(data);
        b.seek(pos);
        b
    }

    #[inline(always)]
    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    pub(crate) fn seek(&mut self, pos: usize) {
        self.pos = pos;
        self.refill();
    }

    /// Makes at least 57 bits available to `peek`.
    #[inline(always)]
    pub(crate) fn refill(&mut self) {
        let byte = self.pos >> 3;
        let word = match self.data.get(byte..byte + 8) {
            Some(w) => u64::from_be_bytes([w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7]]),
            None => {
                let mut w = [0u8; 8];
                if let Some(tail) = self.data.get(byte..) {
                    w[..tail.len()].copy_from_slice(tail);
                }
                u64::from_be_bytes(w)
            }
        };
        self.cache = word << (self.pos & 7);
    }

    /// The next `n` bits (1..=32) without consuming them.
    #[inline(always)]
    pub(crate) fn peek(&self, n: u32) -> u32 {
        (self.cache >> (64 - n)) as u32
    }

    /// The cache's top bit: the next bit.
    #[inline(always)]
    pub(crate) fn top(&self) -> bool {
        (self.cache as i64) < 0
    }

    #[inline(always)]
    pub(crate) fn cache(&self) -> u64 {
        self.cache
    }

    /// Consumes `n` bits (at most what the last refill made available).
    #[inline(always)]
    pub(crate) fn skip(&mut self, n: u32) {
        self.cache = self.cache.checked_shl(n).unwrap_or(0);
        self.pos += n as usize;
    }

    /// Reads `n` bits (0..=32), refilling first.
    pub(crate) fn read(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        self.refill();
        let v = self.peek(n);
        self.skip(n);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_msb_first_across_bytes_and_zeros_past_the_end() {
        let mut b = Bits::new(&[0b1011_0011, 0b0101_1111, 0xFF]);
        assert_eq!(b.read(1), 1);
        assert_eq!(b.read(3), 0b011);
        assert_eq!(b.read(8), 0b0011_0101);
        assert_eq!(b.pos(), 12);
        assert_eq!(b.read(12), 0b1111_1111_1111 & 0xFFF);
        assert_eq!(b.read(32), 0);
        assert_eq!(b.read(0), 0);
        let mut b = Bits::at(&[0x12, 0x34], 4);
        assert_eq!(b.read(8), 0x23);
        b.skip(64);
        assert_eq!(b.cache(), 0);
        assert!(!b.top());
    }
}
