use super::*;

#[test]
fn bits_come_least_significant_first() {
    // The specification's example (section 2.1.6): 4 bits 12, 3 bits -1 (7), 7 bits 17, 13 bits 6969.
    let data = [0b1111_1100, 0b0100_1000, 0b1100_1110, 0b0000_0110];
    let mut r = BitReader::new(&data);
    assert_eq!(r.read(4), Some(12));
    assert_eq!(r.read(3), Some(7));
    assert_eq!(r.read(7), Some(17));
    assert_eq!(r.read(13), Some(6969));
    assert_eq!(r.bits_left(), 5);
}

#[test]
fn reads_up_to_32_bits_at_any_alignment() {
    let data: Vec<u8> = (0..16u8).map(|i| i.wrapping_mul(37).wrapping_add(11)).collect();
    let mut all = 0u128;
    for (i, b) in data.iter().enumerate() {
        all |= u128::from(*b) << (8 * i);
    }
    for start in 0..40u32 {
        for n in 0..=32u32 {
            let mut r = BitReader::new(&data);
            r.read(start % 32).unwrap();
            if start >= 32 {
                r.read(start - 32).unwrap();
            }
            let want = ((all >> start) & ((1u128 << n) - 1)) as u32;
            assert_eq!(r.read(n), Some(want), "start {start} n {n}");
        }
    }
}

#[test]
fn reading_past_the_end_fails_and_leaves_the_reader_exhausted() {
    let data = [0xFF, 0x01];
    let mut r = BitReader::new(&data);
    assert_eq!(r.read(9), Some(0x1FF));
    assert_eq!(r.read(8), None);
    assert_eq!(r.bits_left(), 0);
    assert_eq!(r.read(1), None);
    assert_eq!(r.read(0), Some(0));
    let mut r = BitReader::new(&[]);
    assert_eq!(r.read_bit(), None);
}

#[test]
fn wide_reads_take_up_to_64_bits() {
    let data = [0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x55];
    let mut r = BitReader::new(&data);
    assert_eq!(r.read_wide(4), Some(1));
    assert_eq!(r.read_wide(63), Some(0x5EFC_DAB8_9674_5230));
    let mut r = BitReader::new(&data);
    assert_eq!(r.read_wide(64), Some(0xEFCD_AB89_6745_2301));
    assert_eq!(r.read_wide(9), None);
}

#[test]
fn peeking_pads_with_zeros_and_consuming_past_the_end_exhausts() {
    let data = [0b1010_0101];
    let mut r = BitReader::new(&data);
    assert_eq!(r.peek(12), 0b1010_0101);
    assert!(r.consume(3));
    assert_eq!(r.peek(8), 0b1_0100);
    assert!(!r.consume(6));
    assert_eq!(r.bits_left(), 0);
    assert_eq!(r.peek(8), 0);
}

#[test]
fn ilog_counts_the_bits_of_a_value() {
    assert_eq!(ilog(0), 0);
    assert_eq!(ilog(1), 1);
    assert_eq!(ilog(2), 2);
    assert_eq!(ilog(3), 2);
    assert_eq!(ilog(4), 3);
    assert_eq!(ilog(7), 3);
    assert_eq!(ilog(u32::MAX), 32);
}
