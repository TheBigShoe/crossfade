use super::*;
use crate::celt::cwrs::tests::icwrs;
use crate::celt::test_util::encode;
use crate::range::RangeDecoder;
use crate::testing::range_encoder::Op;

fn norm(x: &[f32]) -> f32 {
    x.iter().map(|v| v * v).sum::<f32>().sqrt()
}

#[test]
fn rotation_is_orthogonal_and_undone_by_its_inverse() {
    let orig: Vec<f32> = (0..32).map(|i| ((i * 37 % 11) as f32) - 5.0).collect();
    for (stride, k, spread) in [(1, 2, 1), (1, 5, 2), (2, 3, 3), (4, 1, 2)] {
        let mut x = orig.clone();
        exp_rotation(&mut x, stride, k, spread);
        assert!((norm(&x) - norm(&orig)).abs() < 1e-3);
        assert!(x != orig);
    }
    // No spreading at SPREAD_NONE or with many pulses.
    let mut x = orig.clone();
    exp_rotation(&mut x, 1, 2, SPREAD_NONE);
    exp_rotation(&mut x, 1, 16, 2);
    assert_eq!(x, orig);
}

#[test]
fn unquant_gives_a_vector_of_the_gain_and_its_collapse_mask() {
    // 8 bins in 4 blocks of 2 (interleaved as the decoder stores them), pulses in blocks 0
    // and 2 only.
    let y = [2, 0, 0, 0, -1, 0, 0, 0];
    let (i, v) = icwrs(&y);
    let frame = encode(&[Op::Uint { val: i, ft: v }]);
    let mut dec = RangeDecoder::new(&frame);
    let mut ec = Ec::new(&mut dec);
    let mut x = [0.0; 8];
    let mask = alg_unquant(&mut x, 3, SPREAD_NONE, 4, &mut ec, 0.5);
    assert_eq!(mask, 0b0101);
    assert!((norm(&x) - 0.5).abs() < 1e-6);
    assert!((x[0] - 0.5 * 2.0 / 5f32.sqrt()).abs() < 1e-6);
}

#[test]
fn renormalise_scales_to_the_gain_and_leaves_zero_finite() {
    let mut x = [3.0, 4.0];
    renormalise_vector(&mut x, 1.0);
    assert!((x[0] - 0.6).abs() < 1e-6 && (x[1] - 0.8).abs() < 1e-6);
    let mut z = [0.0; 4];
    renormalise_vector(&mut z, 1.0);
    assert_eq!(z, [0.0; 4]);
}

/// A deterministic unit vector with a few strong bins.
fn shape(n: usize, seed: u32) -> Vec<f32> {
    let mut s = seed;
    let mut x: Vec<f32> = (0..n)
        .map(|i| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let v = ((s >> 9) as f32 / (1u32 << 23) as f32) - 0.5;
            if i % 5 == 0 { 4.0 * v } else { v }
        })
        .collect();
    let g = 1.0 / norm(&x);
    x.iter_mut().for_each(|v| *v *= g);
    x
}

#[test]
fn quant_codes_a_vector_the_decoder_rebuilds_bit_for_bit() {
    use crate::celt::ec::Enc;
    use crate::range_encoder::RangeEncoder;
    // (n, k, spread, blocks): few pulses, more pulses than half the bins (the projection
    // pre-search), short blocks, every spread.
    for (n, k, spread, b) in [
        (16, 10, 2, 1),
        (4, 12, 2, 1),
        (8, 3, SPREAD_NONE, 4),
        (32, 6, SPREAD_AGGRESSIVE, 2),
        (8, 12, 1, 4),
        (2, 5, 2, 1),
    ] {
        let orig = shape(n, (n * 31 + k) as u32);
        let mut x = orig.clone();
        let mut renc = RangeEncoder::new(128);
        let mut enc = Enc::new(&mut renc);
        let mask = alg_quant(&mut x, k, spread, b, &mut enc, 0.75);
        let rng = renc.rng();
        let frame = renc.done().expect("fits");

        let mut dec = RangeDecoder::new(&frame);
        let mut ec = Ec::new(&mut dec);
        let mut y = vec![0.0; n];
        let dec_mask = alg_unquant(&mut y, k, spread, b, &mut ec, 0.75);
        assert_eq!(dec.rng(), rng, "n {n} k {k}");
        assert_eq!(mask, dec_mask, "n {n} k {k}");
        assert_eq!(
            x, y,
            "the encoder's resynthesis is the decoder's output, n {n} k {k}"
        );
        assert!((norm(&x) - 0.75).abs() < 1e-5);
        let cos: f32 = x.iter().zip(&orig).map(|(a, b)| a * b).sum::<f32>() / 0.75;
        // With more pulses than bins the shape is close; with few it is at least alike.
        let floor = if k >= n { 0.95 } else { 0.5 };
        assert!(cos > floor, "n {n} k {k}: cosine {cos}");
    }
}

#[test]
fn quant_of_silence_or_garbage_still_codes_k_pulses() {
    use crate::celt::ec::Enc;
    use crate::range_encoder::RangeEncoder;
    for fill in [0.0f32, f32::NAN, f32::INFINITY, 1e-30] {
        let mut x = vec![fill; 8];
        let mut renc = RangeEncoder::new(64);
        let mut enc = Enc::new(&mut renc);
        alg_quant(&mut x, 12, 2, 1, &mut enc, 1.0);
        let frame = renc.done().expect("fits");
        let mut dec = RangeDecoder::new(&frame);
        let mut ec = Ec::new(&mut dec);
        let mut y = vec![0.0; 8];
        alg_unquant(&mut y, 12, 2, 1, &mut ec, 1.0);
        assert!(!dec.error());
        assert!((norm(&y) - 1.0).abs() < 1e-5, "fill {fill}");
    }
}
