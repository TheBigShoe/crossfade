use super::*;
use crate::celt::laplace::tests::laplace_symbol;
use crate::celt::test_util::encode;
use crate::range::RangeDecoder;
use crate::testing::range_encoder::Op;

#[test]
fn coarse_energy_predicts_from_the_last_frame_and_the_band_below() {
    // Inter, LM 3, mono, two bands, deltas +2 then -1, from last energies 1.0 and -12.0
    // (clamped to -9 first).
    let model = &E_PROB_MODEL[3][0];
    let mut ops = vec![];
    for (i, q) in [(0usize, 2), (1, -1)] {
        let (fl, fh, _) = laplace_symbol(
            q,
            u32::from(model[2 * i]) << 7,
            u32::from(model[2 * i + 1]) << 6,
        );
        ops.push(Op::Bin { fl, fh, bits: 15 });
    }
    ops.push(Op::Bits { val: 0, bits: 25 });
    ops.push(Op::Bits { val: 0, bits: 25 });
    let frame = encode(&ops);
    let mut dec = RangeDecoder::new(&frame);
    let mut ec = Ec::new(&mut dec);
    let mut old = [0.0; 2 * NB_EBANDS];
    old[0] = 1.0;
    old[1] = -12.0;
    unquant_coarse_energy(&mut old, 0, 2, false, &mut ec, 1, 3);
    let (coef, beta) = (PRED_COEF[3], BETA_COEF[3]);
    let e0 = coef * 1.0 + 0.0 + 2.0;
    let prev = 2.0 - beta * 2.0;
    let e1 = coef * -9.0 + prev - 1.0;
    assert_eq!(old[0], e0);
    assert_eq!(old[1], e1);
}

#[test]
fn out_of_bits_the_delta_is_minus_one() {
    // A frame with no bits left: every band falls by one step plus the prediction.
    let frame = [0u8; 2];
    let mut dec = RangeDecoder::new(&frame);
    let mut ec = Ec::new(&mut dec);
    ec.pretend_read_to(16);
    let mut old = [0.0; 2 * NB_EBANDS];
    unquant_coarse_energy(&mut old, 0, 3, true, &mut ec, 2, 0);
    assert_eq!(old[0], -1.0);
    assert_eq!(old[NB_EBANDS], -1.0);
    assert_eq!(old[1], -1.0 + (-1.0 + BETA_INTRA));
}

#[test]
fn fine_and_final_bits_refine_by_halves() {
    // Band 0 gets 2 fine bits (value 3: +0.375) then a final bit (1: +0.0625).
    let frame = encode(&[Op::Bits { val: 3, bits: 2 }, Op::Bits { val: 1, bits: 1 }]);
    let mut dec = RangeDecoder::new(&frame);
    let mut ec = Ec::new(&mut dec);
    let mut old = [0.0; 2 * NB_EBANDS];
    let mut fine = [0; NB_EBANDS];
    fine[0] = 2;
    unquant_fine_energy(&mut old, 0, 1, &fine, &mut ec, 1);
    assert_eq!(old[0], 0.375);
    unquant_energy_finalise(&mut old, 0, 1, &fine, &[0; NB_EBANDS], 1, &mut ec, 1);
    assert_eq!(old[0], 0.4375);
}

#[test]
fn amplitudes_are_capped_and_zero_outside_the_coded_bands() {
    let mut old = [0.0; 2 * NB_EBANDS];
    old[5] = 1000.0;
    let mut e = [1.0; 2 * NB_EBANDS];
    log2_amp(&mut e, &old, 1, 20, 1);
    assert_eq!(e[0], 0.0);
    assert_eq!(e[20], 0.0);
    assert_eq!(e[5], exp2(32.0));
    assert_eq!(e[1], exp2(E_MEANS[1]));
}
