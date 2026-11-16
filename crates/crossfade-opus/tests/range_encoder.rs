//! The range encoder as the crate's public API: what an encoder needs beyond coding symbols.

use crossfade_opus::{RangeDecoder, RangeEncoder};

const ICDF: [u8; 4] = [200, 120, 30, 0];

#[test]
fn a_shrunk_frame_keeps_its_raw_bits_at_the_new_end() {
    // VBR: the frame's size is chosen after raw bits were already written at the buffer's end.
    let mut enc = RangeEncoder::new(100);
    enc.enc_icdf(2, &ICDF, 8);
    enc.enc_bits(0x2a5, 10);
    enc.enc_bits(0x1f, 5);
    enc.enc_uint(77, 300);
    // 15 raw bits: one whole byte is already in the buffer, at its very end.
    enc.enc_bits(0x155, 9);
    enc.shrink(20);
    assert_eq!(enc.storage(), 20);
    let frame = enc.done().expect("fits in 20 bytes");
    assert_eq!(frame.len(), 20);

    let mut dec = RangeDecoder::new(&frame);
    assert_eq!(dec.dec_icdf(&ICDF, 8), 2);
    assert_eq!(dec.dec_bits(10), 0x2a5);
    assert_eq!(dec.dec_bits(5), 0x1f);
    assert_eq!(dec.dec_uint(300), 77);
    assert_eq!(dec.dec_bits(9), 0x155);
    assert!(!dec.error());
}

#[test]
fn the_first_bits_can_be_patched_after_the_fact() {
    // SILK codes its VAD and LBRR flags as placeholders and patches them once known.
    for later in [0usize, 1, 40] {
        let mut enc = RangeEncoder::new(64);
        for _ in 0..4 {
            enc.enc_bit_logp(false, 1);
        }
        for i in 0..later {
            enc.enc_icdf(i % 4, &ICDF, 8);
        }
        enc.patch_initial_bits(0b10, 2);
        assert!(!enc.error(), "{later} symbols later");
        let frame = enc.done().expect("fits");

        let mut dec = RangeDecoder::new(&frame);
        let flags: Vec<bool> = (0..4).map(|_| dec.dec_bit_logp(1)).collect();
        assert_eq!(flags, [true, false, false, false], "{later} symbols later");
        for i in 0..later {
            assert_eq!(dec.dec_icdf(&ICDF, 8), i % 4);
        }
    }
}

#[test]
fn patching_before_anything_was_coded_is_an_error() {
    let mut enc = RangeEncoder::new(8);
    enc.patch_initial_bits(1, 2);
    assert!(enc.error());
}

#[test]
fn the_bytes_written_so_far_can_be_counted() {
    let mut enc = RangeEncoder::new(64);
    assert_eq!(enc.range_bytes(), 0);
    for _ in 0..64 {
        enc.enc_icdf(3, &ICDF, 8);
    }
    // 64 symbols of about 3 bits each: more than 16 bytes are out, fewer than 32.
    assert!((16..32).contains(&enc.range_bytes()), "{}", enc.range_bytes());
}
