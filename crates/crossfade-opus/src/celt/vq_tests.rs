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

/// A band's worth of unit-norm-ish shapes: smooth, spiky, flat, alternating.
fn shapes(n: usize) -> Vec<Vec<f32>> {
    let mut out = vec![];
    let smooth: Vec<f32> = (0..n).map(|i| libm::sinf(i as f32 * 0.7 + 0.3)).collect();
    let mut spiky = vec![0.01f32; n];
    spiky[n / 3] = -1.0;
    let flat = vec![0.5f32; n];
    let alt: Vec<f32> = (0..n)
        .map(|i| if i % 2 == 0 { 0.3 } else { -0.2 })
        .collect();
    for x in [smooth, spiky, flat, alt] {
        let e = norm(&x);
        out.push(x.iter().map(|v| v / e).collect());
    }
    out
}

#[test]
fn quant_resynthesises_what_unquant_decodes() {
    for (n, k, spread, b) in [
        (2, 1, 2, 1),
        (4, 3, 0, 1),
        (8, 1, 1, 2),
        (8, 20, 2, 4),
        (16, 5, 3, 1),
        (24, 9, 2, 8),
        (32, 6, 2, 1),
        (48, 5, 2, 1),
        (96, 2, 1, 2),
        (176, 4, 2, 1),
    ] {
        for (s, shape) in shapes(n).into_iter().enumerate() {
            let mut x = shape.clone();
            let mut enc = RangeEncoder::new(256);
            let mask = alg_quant(&mut x, k, spread, b, &mut enc, 0.75);
            let frame = enc.done().expect("fits");
            let mut dec = RangeDecoder::new(&frame);
            let mut ec = Ec::new(&mut dec);
            let mut y = vec![0.0f32; n];
            let want = alg_unquant(&mut y, k, spread, b, &mut ec, 0.75);
            assert_eq!(mask, want, "n {n} k {k} shape {s}");
            assert_eq!(x, y, "n {n} k {k} shape {s}");
            // The quantised shape points the way the input did.
            let dot: f32 = shape.iter().zip(&y).map(|(a, b)| a * b).sum::<f32>() / 0.75;
            assert!(dot > 0.2, "n {n} k {k} shape {s}: {dot}");
        }
    }
}

#[test]
fn quant_survives_degenerate_input() {
    for x0 in [[0.0f32; 8], [f32::NAN; 8], [f32::INFINITY; 8], [1e30; 8]] {
        for k in [1, 3, 8, 30] {
            let mut x = x0;
            let mut enc = RangeEncoder::new(64);
            alg_quant(&mut x, k, 2, 1, &mut enc, 1.0);
            let frame = enc.done().expect("fits");
            let mut dec = RangeDecoder::new(&frame);
            let mut ec = Ec::new(&mut dec);
            let mut y = [0.0f32; 8];
            alg_unquant(&mut y, k, 2, 1, &mut ec, 1.0);
            assert_eq!(x, y, "k {k} from {x0:?}");
        }
    }
}

#[test]
fn itheta_is_the_angle_of_side_over_mid() {
    let x: Vec<f32> = (0..16).map(|i| libm::sinf(i as f32)).collect();
    let zero = vec![0.0f32; 16];
    let neg: Vec<f32> = x.iter().map(|v| -v).collect();
    // Mono split: all energy in the first half is 0, all in the second 16384.
    assert_eq!(stereo_itheta(&x, &zero, false), 0);
    assert_eq!(stereo_itheta(&zero, &x, false), 16384);
    assert!((stereo_itheta(&x, &x, false) - 8192).abs() <= 8);
    // Stereo: identical channels are all mid, opposite ones all side.
    assert_eq!(stereo_itheta(&x, &x, true), 0);
    assert_eq!(stereo_itheta(&x, &neg, true), 16384);
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
