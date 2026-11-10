//! MSB-first bit reading and writing, shared by the codecs and bit-packed headers.

use crossfade_core::Error;

pub struct BitReader<'a> {
    data: &'a [u8],
    pos: u64,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn position_bits(&self) -> u64 {
        self.pos
    }

    pub fn remaining_bits(&self) -> u64 {
        (self.data.len() as u64 * 8).saturating_sub(self.pos)
    }

    pub fn is_aligned(&self) -> bool {
        self.pos.is_multiple_of(8)
    }

    fn eof(&self) -> Error {
        Error::UnexpectedEof {
            offset: self.data.len() as u64,
        }
    }

    /// `n` up to 64; more is a caller bug reported as `Malformed`.
    pub fn read_bits(&mut self, n: u32) -> Result<u64, Error> {
        if n > 64 {
            return Err(Error::Malformed {
                what: format!("cannot read {n} bits at once"),
            });
        }
        if u64::from(n) > self.remaining_bits() {
            return Err(self.eof());
        }
        let mut left = n;
        let mut out = 0u64;
        while left > 0 {
            let byte = self.data[(self.pos / 8) as usize];
            let used = (self.pos % 8) as u32;
            let take = (8 - used).min(left);
            let bits = (byte << used) >> (8 - take);
            out = (out << take) | u64::from(bits);
            self.pos += u64::from(take);
            left -= take;
        }
        Ok(out)
    }

    pub fn read_bit(&mut self) -> Result<bool, Error> {
        Ok(self.read_bits(1)? == 1)
    }

    /// Two's complement over `n` bits (`n` up to 64).
    pub fn read_signed(&mut self, n: u32) -> Result<i64, Error> {
        if n == 0 {
            return Ok(0);
        }
        let v = self.read_bits(n)?;
        let shift = 64 - n;
        Ok(((v << shift) as i64) >> shift)
    }

    /// Counts zero bits up to and excluding the terminating one bit.
    pub fn read_unary(&mut self) -> Result<u32, Error> {
        let mut zeros = 0u32;
        while !self.read_bit()? {
            zeros = zeros.saturating_add(1);
        }
        Ok(zeros)
    }

    pub fn skip(&mut self, n: u64) -> Result<(), Error> {
        if n > self.remaining_bits() {
            return Err(self.eof());
        }
        self.pos += n;
        Ok(())
    }

    pub fn byte_align(&mut self) {
        self.pos = self.pos.div_ceil(8) * 8;
    }
}

#[derive(Debug, Default)]
pub struct BitWriter {
    bytes: Vec<u8>,
    /// Bits already used in the last byte of `bytes`, 0 when aligned.
    used: u32,
}

impl BitWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn position_bits(&self) -> u64 {
        let full = self.bytes.len() as u64 * 8;
        if self.used == 0 {
            full
        } else {
            full - 8 + u64::from(self.used)
        }
    }

    /// The low `n` bits of `value`, `n` up to 64.
    pub fn write_bits(&mut self, value: u64, n: u32) {
        debug_assert!(n <= 64);
        let mut left = n;
        while left > 0 {
            if self.used == 0 {
                self.bytes.push(0);
            }
            let room = 8 - self.used;
            let take = room.min(left);
            let chunk = ((value >> (left - take)) & ((1u64 << take) - 1)) as u8;
            let last = self.bytes.len() - 1;
            self.bytes[last] |= chunk << (room - take);
            self.used = (self.used + take) % 8;
            left -= take;
        }
    }

    pub fn write_signed(&mut self, value: i64, n: u32) {
        self.write_bits(value as u64, n);
    }

    /// `zeros` zero bits then a one.
    pub fn write_unary(&mut self, zeros: u32) {
        for _ in 0..zeros {
            self.write_bits(0, 1);
        }
        self.write_bits(1, 1);
    }

    /// Zero padding.
    pub fn byte_align(&mut self) {
        self.used = 0;
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn reads_msb_first() {
        let mut r = BitReader::new(&[0b1010_0101, 0xFF]);
        assert!(r.read_bit().unwrap());
        assert_eq!(r.read_bits(3).unwrap(), 0b010);
        assert_eq!(r.position_bits(), 4);
        assert_eq!(r.read_bits(8).unwrap(), 0b0101_1111);
        assert_eq!(r.remaining_bits(), 4);
        assert_eq!(r.read_bits(0).unwrap(), 0);
    }

    #[test]
    fn reads_up_to_64_bits() {
        let bytes = [0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0, 0x11];
        let mut r = BitReader::new(&bytes);
        r.skip(4).unwrap();
        assert_eq!(r.read_bits(64).unwrap(), 0x2345_6789_ABCD_EF01);
        assert!(r.read_bits(65).is_err());
    }

    #[test]
    fn signed_reads_sign_extend() {
        let mut r = BitReader::new(&[0b1110_0111, 0b1000_0000]);
        assert_eq!(r.read_signed(3).unwrap(), -1);
        assert_eq!(r.read_signed(3).unwrap(), 1);
        assert_eq!(r.read_signed(0).unwrap(), 0);
        assert_eq!(r.read_signed(7).unwrap(), -16);
        let mut r = BitReader::new(&[0x80, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(r.read_signed(64).unwrap(), i64::MIN);
    }

    #[test]
    fn unary_counts_zeros_before_the_one() {
        let mut r = BitReader::new(&[0b0001_1000, 0b0000_0001]);
        assert_eq!(r.read_unary().unwrap(), 3);
        assert_eq!(r.read_unary().unwrap(), 0);
        assert_eq!(r.read_unary().unwrap(), 10);
        assert_eq!(r.position_bits(), 16);
        assert_eq!(
            BitReader::new(&[0, 0]).read_unary(),
            Err(Error::UnexpectedEof { offset: 2 })
        );
    }

    #[test]
    fn skip_and_align() {
        let mut r = BitReader::new(&[0xFF, 0x0F, 0xAA]);
        assert!(r.is_aligned());
        r.skip(3).unwrap();
        assert!(!r.is_aligned());
        r.byte_align();
        assert_eq!(r.position_bits(), 8);
        r.byte_align();
        assert_eq!(r.position_bits(), 8);
        r.skip(8).unwrap();
        assert_eq!(r.read_bits(8).unwrap(), 0xAA);
        assert!(r.skip(1).is_err());
    }

    #[test]
    fn reading_past_the_end_reports_the_byte_offset() {
        let mut r = BitReader::new(&[1, 2]);
        r.skip(12).unwrap();
        assert_eq!(r.read_bits(5), Err(Error::UnexpectedEof { offset: 2 }));
        assert!(!r.read_bit().unwrap());
        assert_eq!(
            BitReader::new(&[]).read_bit(),
            Err(Error::UnexpectedEof { offset: 0 })
        );
    }

    #[test]
    fn writer_packs_msb_first_and_pads_with_zeros() {
        let mut w = BitWriter::new();
        w.write_bits(0b101, 3);
        assert_eq!(w.position_bits(), 3);
        w.write_bits(0xFFFF_FFFF_FFFF_FFFF, 0);
        w.write_bits(0b1_0000_0011, 4); // value is masked to the width
        w.byte_align();
        w.byte_align();
        assert_eq!(w.position_bits(), 8);
        w.write_signed(-2, 4);
        w.write_unary(2);
        w.write_unary(0);
        assert_eq!(w.into_bytes(), [0b1010_0110, 0b1110_0011]);
    }

    #[test]
    fn writer_handles_64_bit_values() {
        let mut w = BitWriter::new();
        w.write_bits(1, 1);
        w.write_bits(u64::MAX, 64);
        let bytes = w.into_bytes();
        assert_eq!(bytes.len(), 9);
        let mut r = BitReader::new(&bytes);
        assert!(r.read_bit().unwrap());
        assert_eq!(r.read_bits(64).unwrap(), u64::MAX);
    }

    fn mask(v: u64, n: u32) -> u64 {
        if n == 64 { v } else { v & ((1u64 << n) - 1) }
    }

    proptest! {
        #[test]
        fn any_write_sequence_reads_back(items in prop::collection::vec((0u32..=64, any::<u64>()), 0..40)) {
            let mut w = BitWriter::new();
            for &(n, v) in &items {
                w.write_bits(v, n);
            }
            let total: u64 = items.iter().map(|&(n, _)| u64::from(n)).sum();
            prop_assert_eq!(w.position_bits(), total);
            let bytes = w.into_bytes();
            let mut r = BitReader::new(&bytes);
            for &(n, v) in &items {
                prop_assert_eq!(r.read_bits(n).unwrap(), mask(v, n));
            }
            r.byte_align();
            prop_assert_eq!(r.remaining_bits(), 0);
        }

        #[test]
        fn signed_and_unary_round_trip(items in prop::collection::vec((1u32..=64, any::<i64>(), 0u32..200), 0..30)) {
            let mut w = BitWriter::new();
            for &(n, v, u) in &items {
                w.write_signed(v, n);
                w.write_unary(u);
            }
            let bytes = w.into_bytes();
            let mut r = BitReader::new(&bytes);
            for &(n, v, u) in &items {
                let expect = if n == 64 { v } else { (v << (64 - n)) >> (64 - n) };
                prop_assert_eq!(r.read_signed(n).unwrap(), expect);
                prop_assert_eq!(r.read_unary().unwrap(), u);
            }
        }
    }
}
