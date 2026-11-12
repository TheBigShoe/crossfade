//! A big-endian bit reader over one packet. Reads past the end fail; peeks past it see zeros.

use crate::{Error, Result};

pub(crate) struct Bits<'a> {
    data: &'a [u8],
    /// Position in bits.
    pos: usize,
    /// `data.len() * 8`.
    end: usize,
}

impl<'a> Bits<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            end: data.len().saturating_mul(8),
        }
    }

    pub(crate) fn position(&self) -> usize {
        self.pos
    }

    /// A reader over the same packet, at `pos` (a position this one has passed).
    pub(crate) fn at(&self, pos: usize) -> Self {
        Self {
            data: self.data,
            pos,
            end: self.end,
        }
    }

    /// The next 57 bits or more at the top of a word, zeros past the end.
    #[inline]
    pub(crate) fn peek64(&self) -> u64 {
        let byte = self.pos >> 3;
        let word = match self.data.get(byte..byte + 8) {
            Some(b) => u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
            None => {
                let mut b = [0u8; 8];
                let tail = self.data.get(byte..).unwrap_or_default();
                b[..tail.len()].copy_from_slice(tail);
                u64::from_be_bytes(b)
            }
        };
        word << (self.pos & 7)
    }

    #[inline]
    pub(crate) fn skip(&mut self, n: usize) -> Result<()> {
        match self.pos.checked_add(n) {
            Some(p) if p <= self.end => {
                self.pos = p;
                Ok(())
            }
            _ => Err(Error::UnexpectedEnd),
        }
    }

    /// `n` bits (at most 57) as an unsigned value.
    #[inline]
    pub(crate) fn read_wide(&mut self, n: u32) -> Result<u64> {
        debug_assert!(n <= 57);
        if n == 0 {
            return Ok(0);
        }
        let v = self.peek64() >> (64 - n);
        self.skip(n as usize)?;
        Ok(v)
    }

    /// `n` bits (at most 32).
    #[inline]
    pub(crate) fn read(&mut self, n: u32) -> Result<u32> {
        debug_assert!(n <= 32);
        Ok(self.read_wide(n)? as u32)
    }

    /// `n` bits (at most 32) as a two's complement value.
    pub(crate) fn read_signed(&mut self, n: u32) -> Result<i32> {
        let v = self.read(n)?;
        Ok(sign_extend(v as i32, n))
    }

    pub(crate) fn align(&mut self) {
        // `end` is a whole number of bytes, so this never passes it.
        self.pos = self.pos.next_multiple_of(8);
    }
}

/// The low `bits` of `v` as a signed value; 32 or more leaves it alone.
#[inline]
pub(crate) fn sign_extend(v: i32, bits: u32) -> i32 {
    if bits >= 32 || bits == 0 {
        return v;
    }
    let shift = 32 - bits;
    (v << shift) >> shift
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_big_endian_and_stops_at_the_end() {
        let mut b = Bits::new(&[0b1010_0000, 0xFF, 0x01]);
        assert_eq!(b.read(3), Ok(0b101));
        assert_eq!(b.read(5), Ok(0));
        assert_eq!(b.read_signed(8), Ok(-1));
        assert_eq!(b.position(), 16);
        assert_eq!(b.read(9), Err(Error::UnexpectedEnd));
        assert_eq!(b.position(), 16, "a failed read moves nothing");
        assert_eq!(b.read(8), Ok(1));
        assert_eq!(b.read(0), Ok(0));
        assert_eq!(b.read(1), Err(Error::UnexpectedEnd));
        assert_eq!(b.peek64(), 0);
    }

    #[test]
    fn wide_reads_and_alignment() {
        let data: Vec<u8> = (1..=12).collect();
        let mut b = Bits::new(&data);
        b.skip(4).unwrap();
        assert_eq!(b.read(32), Ok(0x1020_3040));
        assert_eq!(b.read_wide(33), Ok(0xA0C0_E101));
        b.align();
        assert_eq!(b.position(), 72);
        assert_eq!(b.read(8), Ok(10));
        b.align();
        assert_eq!(b.position(), 80);
        assert_eq!(b.at(4).read(4), Ok(1));
        assert_eq!(b.skip(usize::MAX), Err(Error::UnexpectedEnd));
    }

    #[test]
    fn sign_extension() {
        assert_eq!(sign_extend(0x1_FFFF, 17), -1);
        assert_eq!(sign_extend(0xFFFF, 17), 0xFFFF);
        assert_eq!(sign_extend(-5, 32), -5);
        assert_eq!(sign_extend(0x7FFF_FFFF, 33), 0x7FFF_FFFF);
    }
}
