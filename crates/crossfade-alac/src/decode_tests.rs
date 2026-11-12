use super::*;
use crate::testing::{BitWriter, put_residuals};
use proptest::prelude::*;

fn config(bit_depth: u8, channels: u8, frame_length: u32) -> Config {
    Config {
        frame_length,
        compatible_version: 0,
        bit_depth,
        pb: 40,
        mb: 10,
        kb: 14,
        channels,
        max_run: 255,
        max_frame_bytes: 0,
        avg_bit_rate: 0,
        sample_rate: 44100,
    }
}

fn decode(c: Config, packet: &[u8]) -> Result<Vec<i32>> {
    let mut d = Decoder::new(c)?;
    let mut out = vec![99; 3];
    let frames = d.decode(packet, &mut out)?;
    assert_eq!(out.len(), frames * usize::from(c.channels));
    Ok(out)
}

/// Tag and element header: partial length when given, shifted bytes, escape flag.
fn head(w: &mut BitWriter, tag: u32, partial: Option<u32>, shift: u32, escape: bool) {
    w.put(3, u64::from(tag));
    w.put(4, 0);
    w.put(12, 0);
    w.put(
        4,
        u64::from(partial.is_some()) << 3 | u64::from(shift) << 1 | u64::from(escape),
    );
    if let Some(n) = partial {
        w.put(32, u64::from(n));
    }
}

/// One channel's predictor header.
fn predictor(w: &mut BitWriter, mode: u32, den_shift: u32, pb_factor: u32, coefs: &[i16]) {
    w.put(8, u64::from(mode << 4 | den_shift));
    w.put(8, (u64::from(pb_factor) << 5) | coefs.len() as u64);
    for &c in coefs {
        w.put(16, u64::from(c as u16));
    }
}

fn end(w: &mut BitWriter) -> Vec<u8> {
    w.put(3, u64::from(END));
    std::mem::take(&mut w.bytes)
}

#[test]
fn an_uncompressed_mono_frame() {
    let mut w = BitWriter::default();
    head(&mut w, SCE, Some(3), 0, true);
    for s in [1i16, -2, i16::MIN] {
        w.put(16, u64::from(s as u16));
    }
    assert_eq!(decode(config(16, 1, 4096), &end(&mut w)), Ok(vec![1, -2, -32768]));
}

#[test]
fn an_uncompressed_pair_interleaves_and_a_whole_frame_needs_no_length() {
    let mut w = BitWriter::default();
    head(&mut w, CPE, None, 0, true);
    for (l, r) in [(5, -5), (-8_388_608, 8_388_607)] {
        w.put(24, u64::from(l as u32 & 0xFF_FFFF));
        w.put(24, u64::from(r as u32 & 0xFF_FFFF));
    }
    assert_eq!(
        decode(config(24, 2, 2), &end(&mut w)),
        Ok(vec![5, -5, -8_388_608, 8_388_607])
    );
}

#[test]
fn a_compressed_mono_frame_with_no_predictor() {
    let r = [3, 0, 0, 0, 0, -7, 100, -32768, 32767];
    let mut w = BitWriter::default();
    head(&mut w, SCE, Some(r.len() as u32), 0, false);
    w.put(16, 0); // mix bits and residue
    predictor(&mut w, 0, 9, 4, &[]);
    put_residuals(&mut w, &r, 10, 40, 14, 16);
    assert_eq!(decode(config(16, 1, 4096), &end(&mut w)), Ok(r.to_vec()));
}

#[test]
fn prediction_on_prediction_integrates_before_the_filter() {
    let r = [1, 2, 3, -1, 0];
    // Mode 1 with no taps: one integration. With order 31 (first difference), two.
    for (coefs, want) in [
        (&[][..], vec![1, 3, 6, 5, 5]),
        (&[0i16; 31][..], vec![1, 4, 10, 15, 20]),
    ] {
        let mut w = BitWriter::default();
        head(&mut w, SCE, Some(5), 0, false);
        w.put(16, 0);
        predictor(&mut w, 1, 9, 4, coefs);
        put_residuals(&mut w, &r, 10, 40, 14, 16);
        assert_eq!(decode(config(16, 1, 4096), &end(&mut w)), Ok(want));
    }
}

#[test]
fn a_pair_unmixes_mid_side() {
    // mix_bits 2, mix_res 2: l = u + v - (2 v >> 2), r = l - v.
    let u = [10, -3, 0];
    let v = [4, 5, -6];
    let mut w = BitWriter::default();
    head(&mut w, CPE, Some(3), 0, false);
    w.put(8, 2);
    w.put(8, 2);
    predictor(&mut w, 0, 9, 4, &[]);
    predictor(&mut w, 0, 9, 4, &[]);
    put_residuals(&mut w, &u, 10, 40, 14, 17);
    put_residuals(&mut w, &v, 10, 40, 14, 17);
    let want: Vec<i32> = u
        .iter()
        .zip(&v)
        .flat_map(|(&u, &v)| {
            let l = u + v - ((2 * v) >> 2);
            [l, l - v]
        })
        .collect();
    assert_eq!(want, [12, 8, 0, -5, -3, 3]);
    assert_eq!(decode(config(16, 2, 4096), &end(&mut w)), Ok(want));
}

#[test]
fn shifted_bytes_go_back_under_24_and_32_bit_samples() {
    // 24-bit mono, one byte shifted: 16 coded bits, the low bytes stored before the residuals.
    let mut w = BitWriter::default();
    head(&mut w, SCE, Some(2), 1, false);
    w.put(16, 0);
    predictor(&mut w, 0, 9, 4, &[]);
    w.put(8, 0xAB);
    w.put(8, 0x01);
    put_residuals(&mut w, &[-1, 0x1234], 10, 40, 14, 16);
    assert_eq!(
        decode(config(24, 1, 4096), &end(&mut w)),
        Ok(vec![-0x55, 0x12_3401])
    );
    // 32-bit pair, two bytes shifted: the low halves alternate left, right.
    let mut w = BitWriter::default();
    head(&mut w, CPE, Some(1), 2, false);
    w.put(16, 0);
    predictor(&mut w, 0, 9, 4, &[]);
    predictor(&mut w, 0, 9, 4, &[]);
    w.put(16, 0xBEEF);
    w.put(16, 0x0001);
    put_residuals(&mut w, &[-0x8000], 10, 40, 14, 17);
    put_residuals(&mut w, &[0x7FFF], 10, 40, 14, 17);
    assert_eq!(
        decode(config(32, 2, 4096), &end(&mut w)),
        Ok(vec![i32::MIN | 0xBEEF, 0x7FFF_0001])
    );
}

#[test]
fn at_16_and_20_bits_shifted_bytes_are_dropped() {
    let mut w = BitWriter::default();
    head(&mut w, SCE, Some(1), 1, false);
    w.put(16, 0);
    predictor(&mut w, 0, 9, 4, &[]);
    w.put(8, 0xFF);
    put_residuals(&mut w, &[0x123], 10, 40, 14, 12);
    assert_eq!(decode(config(20, 1, 4096), &end(&mut w)), Ok(vec![0x123]));
}

#[test]
fn samples_wrap_to_the_stream_depth() {
    // A 20-bit stream whose escape values are 20 bits: they come out sign-extended.
    let mut w = BitWriter::default();
    head(&mut w, SCE, Some(2), 0, true);
    w.put(20, 0xF_FFFF);
    w.put(20, 0x7_FFFF);
    assert_eq!(decode(config(20, 1, 4096), &end(&mut w)), Ok(vec![-1, 0x7_FFFF]));
}

#[test]
fn elements_fill_channels_in_order_and_data_and_fill_are_skipped() {
    // 3.0: an SCE (C) and a CPE (L R), with a DSE (aligned, 2 bytes) and a FIL (15 + 2 - 1 bytes)
    // in between. The decoder keeps ALAC's order.
    let mut w = BitWriter::default();
    w.put(3, u64::from(DSE));
    w.put(4, 0);
    w.put(1, 1);
    w.put(8, 2);
    w.align();
    w.put(16, 0xFFFF);
    w.put(3, u64::from(FIL));
    w.put(4, 15);
    w.put(8, 2);
    w.put(16 * 8, u64::MAX);
    head(&mut w, SCE, Some(1), 0, true);
    w.put(16, 7);
    w.put(3, u64::from(FIL));
    w.put(4, 1);
    w.put(8, 0xFF);
    head(&mut w, CPE, Some(1), 0, true);
    w.put(16, 8);
    w.put(16, 9);
    // No END: the last channel ends the packet, whatever follows.
    w.put(3, u64::from(PCE));
    assert_eq!(decode(config(16, 3, 4096), &w.bytes), Ok(vec![7, 8, 9]));
}

#[test]
fn a_long_data_stream_count_and_an_unaligned_one() {
    let mut w = BitWriter::default();
    w.put(3, u64::from(DSE));
    w.put(4, 0);
    w.put(1, 0);
    w.put(8, 255);
    w.put(8, 1);
    w.put(256 * 8 / 64 * 64, 0);
    head(&mut w, LFE, Some(1), 0, true);
    w.put(16, 3);
    assert_eq!(decode(config(16, 1, 4096), &end(&mut w)), Ok(vec![3]));
}

fn fails(c: Config, w: &mut BitWriter) -> Error {
    decode(c, &w.bytes.clone()).unwrap_err()
}

#[test]
fn broken_packets_are_errors() {
    let c = config(16, 2, 8);
    let mut w = BitWriter::default();
    assert_eq!(fails(c, &mut w), Error::UnexpectedEnd);
    for (tag, what) in [
        (CCE, Error::Unsupported("a coupling channel element")),
        (PCE, Error::Unsupported("a program config element")),
    ] {
        let mut w = BitWriter::default();
        w.put(3, u64::from(tag));
        assert_eq!(fails(c, &mut w), what);
    }
    let mut w = BitWriter::default();
    w.put(3, u64::from(END));
    assert_eq!(fails(c, &mut w), Error::Malformed("packet ends before every channel"));
    let mut w = BitWriter::default();
    head(&mut w, CPE, Some(1), 0, true);
    w.put(32, 0);
    head(&mut w, CPE, Some(1), 0, true);
    let three = config(16, 3, 8);
    assert_eq!(
        fails(three, &mut w),
        Error::Malformed("more channels than the config has")
    );
    let mut w = BitWriter::default();
    w.put(3, 0);
    w.put(4, 0);
    w.put(12, 1);
    w.put(4, 0);
    assert_eq!(fails(c, &mut w), Error::Malformed("reserved header bits are set"));
    let mut w = BitWriter::default();
    head(&mut w, CPE, Some(9), 0, true);
    assert_eq!(
        fails(c, &mut w),
        Error::Malformed("frame longer than the config's frame length")
    );
    let mut w = BitWriter::default();
    head(&mut w, SCE, Some(1), 0, true);
    w.put(16, 0);
    head(&mut w, SCE, Some(2), 0, true);
    assert_eq!(
        fails(c, &mut w),
        Error::Malformed("elements of one packet differ in length")
    );
    let mut w = BitWriter::default();
    head(&mut w, SCE, Some(1), 2, false);
    assert_eq!(
        fails(c, &mut w),
        Error::Malformed("shifted bytes leave no bits to code")
    );
    let mut w = BitWriter::default();
    head(&mut w, SCE, Some(4), 0, true);
    w.put(16, 1);
    assert_eq!(fails(config(16, 1, 8), &mut w), Error::UnexpectedEnd);
}

#[test]
fn an_error_leaves_nothing_behind_and_the_decoder_goes_on() {
    let c = config(16, 1, 8);
    let mut d = Decoder::new(c).unwrap();
    let mut out = Vec::new();
    assert!(d.decode(&[0xE0], &mut out).is_err());
    assert!(out.is_empty());
    let mut w = BitWriter::default();
    head(&mut w, SCE, Some(1), 0, true);
    w.put(16, 4);
    assert_eq!(d.decode(&end(&mut w), &mut out), Ok(1));
    assert_eq!(out, [4]);
    assert_eq!(d.config(), &c);
    assert!(Decoder::new(config(8, 1, 8)).is_err());
}

proptest! {
    #[test]
    fn hostile_packets_never_panic(
        packet in prop::collection::vec(any::<u8>(), 0..300),
        depth in prop::sample::select(vec![16u8, 20, 24, 32]),
        channels in 1u8..=8,
        frame_length in 1u32..64,
        pb in any::<u8>(), mb in any::<u8>(), kb in 1u8..=31,
    ) {
        let c = Config { pb, mb, kb, ..config(depth, channels, frame_length) };
        let mut d = Decoder::new(c).unwrap();
        let mut out = Vec::new();
        if let Ok(frames) = d.decode(&packet, &mut out) {
            prop_assert!(frames <= frame_length as usize);
            prop_assert_eq!(out.len(), frames * usize::from(channels));
            let half = 1i64 << (depth - 1);
            prop_assert!(out.iter().all(|&s| (-half..half).contains(&i64::from(s))));
        }
    }
}
