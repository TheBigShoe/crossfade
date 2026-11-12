//! MSB-first bit reading. Reading past the end yields zero bits instead of failing, so hot
//! loops carry no error paths; callers check [`BitReader::overrun`] once per syntactic
//! element. Every loop in the syntax is bounded by counts, so zeros cannot make one endless.

pub(crate) struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// The next 32 bits, left-aligned, zero past the end.
    #[inline]
    pub(crate) fn peek32(&self) -> u32 {
        let byte = self.pos >> 3;
        let word = match self.data.get(byte..byte + 8) {
            Some(b) => u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
            None => {
                let mut b = [0u8; 8];
                if let Some(rest) = self.data.get(byte..) {
                    b[..rest.len()].copy_from_slice(rest);
                }
                u64::from_be_bytes(b)
            }
        };
        ((word << (self.pos & 7)) >> 32) as u32
    }

    /// `n` bits (0 to 32) as an unsigned number.
    #[inline]
    pub(crate) fn read(&mut self, n: u32) -> u32 {
        debug_assert!(n <= 32);
        if n == 0 {
            return 0;
        }
        let v = self.peek32() >> (32 - n);
        self.pos += n as usize;
        v
    }

    #[inline]
    pub(crate) fn bit(&mut self) -> bool {
        self.read(1) != 0
    }

    #[inline]
    pub(crate) fn skip(&mut self, n: usize) {
        self.pos = self.pos.saturating_add(n);
    }

    pub(crate) fn len_bits(&self) -> usize {
        self.data.len() * 8
    }

    /// Bits left before the end; 0 once past it.
    pub(crate) fn remaining(&self) -> usize {
        self.len_bits().saturating_sub(self.pos)
    }

    /// Whether more bits were consumed than the input has.
    pub(crate) fn overrun(&self) -> bool {
        self.pos > self.len_bits()
    }

    /// Skips to the next byte boundary, counted from the start of the input.
    pub(crate) fn byte_align(&mut self) {
        self.pos = (self.pos + 7) & !7;
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
}
