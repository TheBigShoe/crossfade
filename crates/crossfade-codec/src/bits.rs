#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn reads_msb_first() {
        let mut r = BitReader::new(&[0b1010_0101, 0xFF]);
        assert_eq!(r.read_bit().unwrap(), true);
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
        assert_eq!(r.read_signed(7).unwrap(), -64);
        let mut r = BitReader::new(&[0x80, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(r.read_signed(64).unwrap(), i64::MIN);
    }

    #[test]
    fn unary_counts_zeros_before_the_one() {
        let mut r = BitReader::new(&[0b0001_1000, 0b0000_0001]);
        assert_eq!(r.read_unary().unwrap(), 3);
        assert_eq!(r.read_unary().unwrap(), 0);
        assert_eq!(r.read_unary().unwrap(), 7);
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
        assert_eq!(r.read_bit().unwrap(), false);
        assert_eq!(BitReader::new(&[]).read_bit(), Err(Error::UnexpectedEof { offset: 0 }));
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
        assert_eq!(w.into_bytes(), [0b1010_0110, 0b1110_0110]);
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
