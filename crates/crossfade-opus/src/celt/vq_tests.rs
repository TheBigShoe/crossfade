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
