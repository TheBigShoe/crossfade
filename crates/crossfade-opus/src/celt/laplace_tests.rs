use super::*;
use crate::celt::test_util::encode;
use crate::range::RangeDecoder;
use crate::testing::range_encoder::Op;

/// `ec_laplace_encode`'s symbol for `val`: `(fl, fh)` of 32768, and the value it codes
/// (clamped where the tail runs out).
pub(in crate::celt) fn laplace_symbol(val: i32, mut fs: u32, decay: u32) -> (u32, u32, i32) {
    let mut fl = 0u32;
    let mut value = val;
    if val != 0 {
        let s: i32 = -i32::from(val < 0);
        let v = (val + s) ^ s;
        fl = fs;
        fs = freq1(fs, decay);
        let mut i = 1;
        while fs > 0 && i < v {
            fs *= 2;
            fl += fs + 2 * MINP;
            fs = (fs * decay) >> 15;
            i += 1;
        }
        if fs == 0 {
            let ndi_max = (32768 - fl + MINP - 1) as i32;
            let ndi_max = (ndi_max - s) >> 1;
            let di = (v - i).min(ndi_max - 1);
            fl = (fl as i32 + (2 * di + 1 + s) * MINP as i32) as u32;
            fs = MINP.min(32768 - fl);
            value = (i + di + s) ^ s;
        } else {
            fs += MINP;
            fl += fs & !(s as u32);
        }
    }
    (fl, fl + fs, value)
}

#[test]
fn decodes_what_the_reference_encoder_writes() {
    // Every LM/intra/band model of the coarse energy, every delta in -40..40.
    for (fs, decay) in [(72 << 7, 127 << 6), (24 << 7, 179 << 6), (192 << 7, 6 << 6)] {
        let mut ops = vec![];
        let mut want = vec![];
        for val in -40..40 {
            let (fl, fh, v) = laplace_symbol(val, fs, decay);
            assert!(fl < fh && fh <= 32768, "{val} {fs} {decay}: {fl} {fh}");
            ops.push(Op::Bin { fl, fh, bits: 15 });
            want.push(v);
        }
        let frame = encode(&ops);
        let mut dec = RangeDecoder::new(&frame);
        let mut ec = Ec::new(&mut dec);
        let got: Vec<i32> = want.iter().map(|_| decode(&mut ec, fs, decay)).collect();
        assert_eq!(got, want, "fs {fs} decay {decay}");
        assert!(!dec.error());
    }
}

#[test]
fn any_bytes_decode_without_panic() {
    let frame: Vec<u8> = (0..64u32)
        .map(|i| (i.wrapping_mul(97) ^ 0xA5) as u8)
        .collect();
    let mut dec = RangeDecoder::new(&frame);
    let mut ec = Ec::new(&mut dec);
    for _ in 0..200 {
        let _ = decode(&mut ec, 255 << 7, 255 << 6);
        let _ = decode(&mut ec, 0, 0);
    }
}
