use super::*;
use crate::testing::range_encoder::{Op, RangeEncoder, encode_ops};
use proptest::prelude::*;

/// RFC 6716 section 4.1.6.2, as written: three squarings of the Q15 fraction of `rng`.
fn rfc_tell_frac(nbits_total: i32, rng: u32) -> u32 {
    let nbits = (nbits_total as u32) << 3;
    let mut l = ilog(rng);
    let mut r = rng >> (l - 16);
    for _ in 0..3 {
        r = (r * r) >> 15;
        let b = r >> 16;
        l = (l << 1) | b;
        r >>= b;
    }
    nbits.wrapping_sub(l)
}

/// Decodes `op` as the encoder coded it, returning whether the value came back.
fn decode_op(dec: &mut RangeDecoder<'_>, op: &Op) -> bool {
    match op {
        Op::Symbol { fl, fh, ft } => {
            let fs = dec.decode(*ft);
            let ok = (*fl..*fh).contains(&fs);
            dec.update(*fl, *fh, *ft);
            ok
        }
        Op::Bin { fl, fh, bits } => {
            let fs = dec.decode_bin(*bits);
            let ok = (*fl..*fh).contains(&fs);
            dec.update(*fl, *fh, 1 << bits);
            ok
        }
        Op::BitLogp { val, logp } => dec.dec_bit_logp(*logp) == *val,
        Op::Icdf { s, icdf, ftb } => dec.dec_icdf(icdf, *ftb) == *s,
        Op::Icdf16 { s, icdf, ftb } => dec.dec_icdf16(icdf, *ftb) == *s,
        Op::Uint { val, ft } => dec.dec_uint(*ft) == *val,
        Op::Bits { val, bits } => dec.dec_bits(*bits) == *val,
    }
}

#[test]
fn a_new_decoder_has_used_one_bit() {
    // RFC 6716 4.1.6.1: "In a newly initialized decoder ... 1 bit has been used."
    for buf in [&[][..], &[0x00], &[0xFF, 0x12, 0x34]] {
        let dec = RangeDecoder::new(buf);
        assert_eq!(dec.tell(), 1);
        assert_eq!(dec.tell_frac(), 8);
        // Initialised to 128, then renormalised three times to above 2**23.
        assert_eq!(dec.rng(), 1 << 31);
        assert!(!dec.error());
    }
}

#[test]
fn an_empty_frame_decodes_the_lowest_symbol_and_0xff_the_highest() {
    // Zeros give val = 0x7FFFFFFF (the bottom of the range), 0xFF bytes give val = 0.
    let mut zeros = RangeDecoder::new(&[]);
    assert_eq!(zeros.decode(10), 0);
    let mut ones = RangeDecoder::new(&[0xFF, 0xFF, 0xFF, 0xFF]);
    assert_eq!(ones.decode(10), 9);
    assert!(!RangeDecoder::new(&[]).dec_bit_logp(1));
}

#[test]
fn the_celt_silence_frame_decodes_silence() {
    // opus_decoder.c decodes {0xFF, 0xFF} through CELT as silence: its first symbol,
    // ec_dec_bit_logp(dec, 15) at tell == 1, is a 1.
    let mut dec = RangeDecoder::new(&[0xFF, 0xFF]);
    assert!(dec.dec_bit_logp(15));
}

#[test]
fn raw_bits_are_read_lsb_first_from_the_end() {
    // RFC 6716 4.1.4: the first value's LSB in the last byte's LSB, on to the penultimate byte.
    let buf = [0x3C, 0x81, 0xA5];
    let mut dec = RangeDecoder::new(&buf);
    let before = dec.tell();
    assert_eq!(dec.dec_bits(4), 0x5);
    assert_eq!(dec.dec_bits(4), 0xA);
    assert_eq!(dec.dec_bits(9), 0x081);
    assert_eq!(dec.tell(), before + 17);
    assert_eq!(dec.dec_bits(7), 0x3C >> 1);
    // Past the start of the frame: zeros.
    assert_eq!(dec.dec_bits(25), 0);
    assert_eq!(dec.dec_bits(0), 0);
}

#[test]
fn dec_uint_splits_large_totals_into_a_symbol_and_raw_bits() {
    // ft = 2**20: the top 8 bits range coded (ft' = 256), the low 12 raw.
    let mut enc = RangeEncoder::new(16);
    enc.encode(0xAB, 0xAC, 256);
    enc.enc_bits(0xCDE, 12);
    let frame = enc.done().unwrap();
    let mut dec = RangeDecoder::new(&frame);
    assert_eq!(dec.dec_uint(1 << 20), 0xABCDE);
    assert!(!dec.error());
}

#[test]
fn dec_uint_out_of_range_sets_the_error_flag_and_saturates() {
    // ft = 257: ftb = 9, so the symbol has total 129 and one raw bit follows; 128 then 1 gives
    // 257, past ft - 1.
    let mut enc = RangeEncoder::new(16);
    enc.encode(128, 129, 129);
    enc.enc_bits(1, 1);
    let frame = enc.done().unwrap();
    let mut dec = RangeDecoder::new(&frame);
    assert_eq!(dec.dec_uint(257), 256);
    assert!(dec.error());
}

#[test]
fn shrinking_moves_the_raw_bits_to_the_new_end() {
    // Hybrid redundancy: the top level takes bytes off the end before CELT reads raw bits.
    let buf = [0x00, 0x00, 0x5A, 0xFF, 0xFF];
    let mut dec = RangeDecoder::new(&buf);
    assert_eq!(dec.storage(), 5);
    dec.shrink(2);
    assert_eq!(dec.storage(), 3);
    assert_eq!(dec.dec_bits(8), 0x5A);
    // Shrinking past the start leaves an empty frame, not a panic.
    dec.shrink(100);
    assert_eq!(dec.storage(), 0);
    assert_eq!(dec.dec_bits(8), 0);
}

#[test]
fn tell_frac_equals_the_rfc_procedure_for_every_fraction() {
    // The table form (libopus) against the three squarings (RFC 6716 4.1.6.2), for every
    // 16-bit fraction at every exponent the decoder can hold.
    for l in 24..=32u32 {
        for r in 32768u32..65536 {
            let rng = (r << (l - 16)) | ((1 << (l - 16)) - 1);
            for nbits in [9, 33, 1000] {
                assert_eq!(
                    tell_frac(nbits, rng),
                    rfc_tell_frac(nbits, rng),
                    "rng {rng:#x}"
                );
            }
        }
    }
}

#[test]
fn the_range_bytes_count_what_the_range_coder_read() {
    let dec = RangeDecoder::new(&[1, 2, 3, 4, 5, 6]);
    // Initialisation reads the first byte and three more while renormalising.
    assert_eq!(dec.range_bytes(), 4);
}

fn op() -> impl Strategy<Value = Op> {
    let symbol = (1u32..=65535)
        .prop_flat_map(|ft| (0..ft, Just(ft)))
        .prop_flat_map(|(fl, ft)| (Just(fl), fl + 1..=ft, Just(ft)))
        .prop_map(|(fl, fh, ft)| Op::Symbol { fl, fh, ft });
    let bin = (0u32..=15)
        .prop_flat_map(|bits| (0..1u32 << bits, Just(bits)))
        .prop_flat_map(|(fl, bits)| (Just(fl), fl + 1..=1 << bits, Just(bits)))
        .prop_map(|(fl, fh, bits)| Op::Bin { fl, fh, bits });
    let logp = (any::<bool>(), 1u32..=15).prop_map(|(val, logp)| Op::BitLogp { val, logp });
    let icdf = (1u32..=8)
        .prop_flat_map(|ftb| (Just(ftb), proptest::collection::btree_set(1u32..1 << ftb, 0..12)))
        .prop_flat_map(|(ftb, set)| {
            let mut t: Vec<u8> = set.into_iter().rev().map(|v| v as u8).collect();
            t.push(0);
            (0..t.len(), Just(t), Just(ftb))
        })
        .prop_map(|(s, icdf, ftb)| Op::Icdf { s, icdf, ftb });
    let icdf16 = (1u32..=15)
        .prop_flat_map(|ftb| (Just(ftb), proptest::collection::btree_set(1u32..1 << ftb, 0..12)))
        .prop_flat_map(|(ftb, set)| {
            let mut t: Vec<u16> = set.into_iter().rev().map(|v| v as u16).collect();
            t.push(0);
            (0..t.len(), Just(t), Just(ftb))
        })
        .prop_map(|(s, icdf, ftb)| Op::Icdf16 { s, icdf, ftb });
    let uint = (2u32..=u32::MAX)
        .prop_flat_map(|ft| (0..ft, Just(ft)))
        .prop_map(|(val, ft)| Op::Uint { val, ft });
    let bits = (0u32..=25)
        .prop_flat_map(|bits| (0..1u32 << bits, Just(bits)))
        .prop_map(|(val, bits)| Op::Bits { val, bits });
    prop_oneof![symbol, bin, logp, icdf, icdf16, uint, bits]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn every_primitive_round_trips_with_the_same_tell_and_range(
        ops in proptest::collection::vec(op(), 0..200),
    ) {
        let (steps, frame) = encode_ops(&ops);
        let mut dec = RangeDecoder::new(&frame);
        for (i, (op, &(tell, frac, rng))) in ops.iter().zip(&steps).enumerate() {
            prop_assert!(decode_op(&mut dec, op), "op {i}: {op:?}");
            prop_assert_eq!(dec.tell(), tell, "tell after op {}", i);
            prop_assert_eq!(dec.tell_frac(), frac, "tell_frac after op {}", i);
            prop_assert_eq!(dec.rng(), rng, "rng after op {}", i);
            // ec_tell() is ceil(ec_tell_frac() / 8) (RFC 6716 4.1.6).
            prop_assert_eq!(dec.tell() as u32, dec.tell_frac().div_ceil(8));
        }
        prop_assert!(!dec.error());
        // The encoder's budget: never more bits used than the frame holds.
        prop_assert!(dec.tell() as usize <= frame.len() * 8 || ops.is_empty());
    }

    #[test]
    fn range_decoding_reads_zeros_past_the_end(
        bytes in proptest::collection::vec(any::<u8>(), 0..64),
        fts in proptest::collection::vec(1u32..=65535, 0..64),
    ) {
        // Without raw bits, a frame decodes the same with zeros appended.
        let mut padded = bytes.clone();
        padded.resize(bytes.len() + 64, 0);
        let mut a = RangeDecoder::new(&bytes);
        let mut b = RangeDecoder::new(&padded);
        for &ft in &fts {
            let (x, y) = (a.decode(ft), b.decode(ft));
            prop_assert_eq!(x, y);
            a.update(x, x + 1, ft);
            b.update(y, y + 1, ft);
            prop_assert_eq!(a.rng(), b.rng());
            prop_assert_eq!(a.tell_frac(), b.tell_frac());
        }
    }

    #[test]
    fn any_bytes_and_any_arguments_never_panic(
        bytes in proptest::collection::vec(any::<u8>(), 0..32),
        calls in proptest::collection::vec((0u8..9, any::<u32>(), any::<u32>(), any::<u32>()), 0..64),
        table in proptest::collection::vec(any::<u8>(), 0..8),
    ) {
        let table16: Vec<u16> = table.iter().map(|&v| u16::from(v) * 257).collect();
        let mut dec = RangeDecoder::new(&bytes);
        for (kind, a, b, c) in calls {
            match kind {
                0 => { dec.decode(a); }
                1 => { dec.decode_bin(a); }
                2 => dec.update(a, b, c),
                3 => { dec.dec_bit_logp(a); }
                4 => { dec.dec_icdf(&table, a); }
                5 => { dec.dec_icdf16(&table16, a); }
                6 => { dec.dec_uint(a); }
                7 => { dec.dec_bits(a); }
                _ => dec.shrink(a as usize),
            }
            let _ = (dec.tell(), dec.tell_frac(), dec.rng(), dec.error(), dec.storage());
        }
    }
}
