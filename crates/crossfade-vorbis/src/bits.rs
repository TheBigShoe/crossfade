//! Vorbis packs bits least significant first (specification section 2). Reading past the end
//! of a packet fails and leaves the reader exhausted, so every later read fails too, as in
//! libvorbis.

#[derive(Debug, Clone)]
pub(crate) struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    len: usize,
}

impl<'a> BitReader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            len: data.len().saturating_mul(8),
        }
    }

    pub(crate) fn bits_left(&self) -> usize {
        self.len - self.pos
    }

    /// At least the next 57 bits, zero past the end, without consuming them.
    #[inline(always)]
    fn window(&self) -> u64 {
        let byte = self.pos >> 3;
        let word = match self.data.get(byte..byte + 8) {
            Some(s) => u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]),
            None => {
                let mut buf = [0u8; 8];
                let rest = self.data.get(byte..).unwrap_or(&[]);
                buf[..rest.len()].copy_from_slice(rest);
                u64::from_le_bytes(buf)
            }
        };
        word >> (self.pos & 7)
    }

    /// The next `n` bits (at most 32), zero past the end, without consuming them.
    #[inline(always)]
    pub(crate) fn peek(&self, n: u32) -> u32 {
        debug_assert!(n <= 32);
        (self.window() & mask(n)) as u32
    }

    /// Consumes `n` bits; false (and exhausted) if fewer were left.
    #[inline(always)]
    pub(crate) fn consume(&mut self, n: u32) -> bool {
        let n = n as usize;
        if n > self.len - self.pos {
            self.pos = self.len;
            return false;
        }
        self.pos += n;
        true
    }

    #[inline(always)]
    pub(crate) fn read(&mut self, n: u32) -> Option<u32> {
        let v = self.peek(n);
        self.consume(n).then_some(v)
    }

    pub(crate) fn read_bit(&mut self) -> Option<bool> {
        self.read(1).map(|b| b == 1)
    }

    /// Up to 64 bits.
    pub(crate) fn read_wide(&mut self, n: u32) -> Option<u64> {
        if n <= 32 {
            return self.read(n).map(u64::from);
        }
        let lo = self.read(32)?;
        let hi = self.read(n - 32)?;
        Some(u64::from(lo) | (u64::from(hi) << 32))
    }
}

#[inline(always)]
fn mask(n: u32) -> u64 {
    if n >= 64 { u64::MAX } else { (1u64 << n) - 1 }
}

/// The number of bits needed to hold `v`: the specification's `ilog`.
pub(crate) fn ilog(v: u32) -> u32 {
    32 - v.leading_zeros()
}

#[cfg(test)]
#[path = "bits_tests.rs"]
mod tests;
