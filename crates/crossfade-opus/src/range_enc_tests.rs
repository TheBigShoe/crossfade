use super::*;
use crate::RangeDecoder;

/// Three placeholder bits, `filler` more symbols, then the bits patched to `value`.
fn patched(filler: usize, value: u32) -> Vec<u8> {
    let mut enc = RangeEncoder::new(64);
    for _ in 0..3 {
        enc.enc_bit_logp(false, 1);
    }
    for i in 0..filler {
        enc.enc_uint(i as u32 % 7, 7);
    }
    enc.patch_initial_bits(value, 3);
    enc.done().expect("fits")
}

#[test]
fn patch_initial_bits_rewrites_the_first_bits_and_nothing_else() {
    // No renormalisation yet, a first byte held back for a carry, a first byte written.
    for filler in [0, 2, 40] {
        for value in 0..8 {
            let frame = patched(filler, value);
            let mut dec = RangeDecoder::new(&frame);
            let bits = (0..3).fold(0, |acc, _| acc << 1 | u32::from(dec.dec_bit_logp(1)));
            assert_eq!(bits, value, "filler {filler}");
            for i in 0..filler {
                assert_eq!(
                    dec.dec_uint(7),
                    i as u32 % 7,
                    "filler {filler} value {value}"
                );
            }
            assert!(!dec.error());
        }
    }
}

#[test]
fn patch_initial_bits_before_enough_was_coded_is_an_error() {
    let mut enc = RangeEncoder::new(8);
    enc.enc_bit_logp(false, 1);
    enc.patch_initial_bits(3, 3);
    assert!(enc.error());
}

#[test]
fn shrink_gives_the_frame_a_smaller_buffer_would_have() {
    let code = |enc: &mut RangeEncoder| {
        for i in 0..20 {
            enc.enc_uint(i % 5, 5);
            enc.enc_bits(i, 5);
        }
    };
    let mut small = RangeEncoder::new(40);
    code(&mut small);
    let mut big = RangeEncoder::new(1000);
    code(&mut big);
    big.shrink(40);
    assert_eq!(big.done(), small.done());
}
