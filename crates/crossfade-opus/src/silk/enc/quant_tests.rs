use super::*;
use crate::silk::decode_parameters::gains_dequant;

#[test]
fn quantised_gains_are_what_the_decoder_dequantises() {
    let mut s = 12345u32;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        s
    };
    for case in 0..400 {
        let nb_subfr = if case % 2 == 0 { 4 } else { 2 };
        let conditional = case % 3 == 0;
        let mut gains = [0i32; MAX_NB_SUBFR];
        for g in gains.iter_mut().take(nb_subfr) {
            // From near silence to full scale, with jumps that need the double step.
            *g = 1 + (next() >> (next() % 31)) as i32 / 2;
        }
        let prev0 = (next() % 64) as i8;
        let mut ind = [0i8; MAX_NB_SUBFR];
        let mut prev_enc = prev0;
        let mut q = gains;
        gains_quant(&mut ind, &mut q, &mut prev_enc, conditional, nb_subfr);
        let mut deq = [0i32; MAX_NB_SUBFR];
        let mut prev_dec = prev0;
        gains_dequant(&mut deq, &ind, &mut prev_dec, conditional, nb_subfr);
        assert_eq!(q[..nb_subfr], deq[..nb_subfr], "case {case}");
        assert_eq!(prev_enc, prev_dec, "case {case}");
        let first_max = if conditional { 40 } else { 63 };
        assert!((0..=first_max).contains(&ind[0]), "case {case}: {ind:?}");
        assert!(ind[1..nb_subfr].iter().all(|&i| (0..=40).contains(&i)));
    }
}

#[test]
fn a_steady_gain_is_quantised_within_a_step() {
    let mut prev = 0i8;
    for _ in 0..8 {
        let mut ind = [0i8; MAX_NB_SUBFR];
        let mut g = [1 << 20; MAX_NB_SUBFR];
        gains_quant(&mut ind, &mut g, &mut prev, false, 4);
        for &q in &g {
            let ratio = f64::from(q) / f64::from(1 << 20);
            assert!((0.8..1.25).contains(&ratio), "{ratio}");
        }
    }
}

#[test]
fn the_gains_id_tells_index_vectors_apart() {
    let a = gains_id(&[1, 2, 3, 4], 4);
    assert_ne!(a, gains_id(&[1, 2, 4, 3], 4));
    assert_eq!(a, gains_id(&[1, 2, 3, 4], 4));
    assert_eq!(gains_id(&[1, 2, 9, 9], 2), gains_id(&[1, 2, 0, 0], 2));
}

#[test]
fn the_ltp_quantiser_finds_a_codebook_filter_from_its_correlations() {
    // Uncorrelated taps (XX = I) and xX = the target filter: the best vector is the target.
    let cb = 2usize;
    let target = 17usize;
    let one_q17 = 1 << 17;
    let mut xx = [0i32; MAX_NB_SUBFR * LTP_ORDER * LTP_ORDER];
    let mut x_x = [0i32; MAX_NB_SUBFR * LTP_ORDER];
    for j in 0..MAX_NB_SUBFR {
        for k in 0..LTP_ORDER {
            xx[j * 25 + k * 6] = one_q17;
            x_x[j * 5 + k] = i32::from(LTP_VQ_Q7[cb][target][k]) << 10;
        }
    }
    let mut b = [0i16; MAX_NB_SUBFR * LTP_ORDER];
    let mut sum_log_gain = 0;
    let c = quant_ltp_gains(&mut b, &mut sum_log_gain, &xx, &x_x, 80, 4);
    assert_eq!(c.periodicity_index, cb as i8);
    assert_eq!(c.cbk_index, [target as i8; 4]);
    for j in 0..4 {
        for k in 0..5 {
            assert_eq!(b[j * 5 + k], i16::from(LTP_VQ_Q7[cb][target][k]) << 7);
        }
    }
    assert!(c.pred_gain_db_q7 > 0);
}
