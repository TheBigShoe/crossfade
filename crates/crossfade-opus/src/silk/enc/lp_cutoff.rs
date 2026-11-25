//! The low-pass filter of a bandwidth switch (libopus `silk/LP_variable_cutoff.c`,
//! `biquad_alt.c`): elliptic filters, interpolated as the transition runs, so that the coded
//! bandwidth narrows or widens smoothly.

use super::fix::{lshift, rshift_round, sat16, smlawb, smulwb};
use super::state::LpState;

pub(super) const TRANSITION_FRAMES: i32 = 5120 / 20;
const TRANSITION_INT_NUM: usize = 5;

static TRANSITION_LP_B_Q28: [[i32; 3]; TRANSITION_INT_NUM] = [
    [250_767_114, 501_534_038, 250_767_114],
    [209_867_381, 419_732_057, 209_867_381],
    [170_987_846, 341_967_853, 170_987_846],
    [131_531_482, 263_046_905, 131_531_482],
    [89_306_658, 178_584_282, 89_306_658],
];
static TRANSITION_LP_A_Q28: [[i32; 2]; TRANSITION_INT_NUM] = [
    [506_393_414, 239_854_379],
    [411_067_935, 169_683_996],
    [306_733_530, 116_694_253],
    [185_807_084, 77_959_395],
    [35_497_197, 57_401_098],
];

/// `silk_LP_interpolate_filter_taps`.
fn interpolate_filter_taps(ind: usize, fac_q16: i32) -> ([i32; 3], [i32; 2]) {
    if ind >= TRANSITION_INT_NUM - 1 {
        return (
            TRANSITION_LP_B_Q28[TRANSITION_INT_NUM - 1],
            TRANSITION_LP_A_Q28[TRANSITION_INT_NUM - 1],
        );
    }
    if fac_q16 <= 0 {
        return (TRANSITION_LP_B_Q28[ind], TRANSITION_LP_A_Q28[ind]);
    }
    let (b0, b1) = (&TRANSITION_LP_B_Q28[ind], &TRANSITION_LP_B_Q28[ind + 1]);
    let (a0, a1) = (&TRANSITION_LP_A_Q28[ind], &TRANSITION_LP_A_Q28[ind + 1]);
    let mut b = [0i32; 3];
    let mut a = [0i32; 2];
    if fac_q16 < 32768 {
        for k in 0..3 {
            b[k] = smlawb(b0[k], b1[k] - b0[k], fac_q16);
        }
        for k in 0..2 {
            a[k] = smlawb(a0[k], a1[k] - a0[k], fac_q16);
        }
    } else {
        for k in 0..3 {
            b[k] = smlawb(b1[k], b1[k] - b0[k], fac_q16 - (1 << 16));
        }
        for k in 0..2 {
            a[k] = smlawb(a1[k], a1[k] - a0[k], fac_q16 - (1 << 16));
        }
    }
    (b, a)
}

/// `silk_biquad_alt_stride1`: a second-order ARMA filter in place.
fn biquad_alt_stride1(x: &mut [i16], b_q28: &[i32; 3], a_q28: &[i32; 2], s: &mut [i32; 2]) {
    let a0_l_q28 = a_q28[0].wrapping_neg() & 0x0000_3FFF;
    let a0_u_q28 = a_q28[0].wrapping_neg() >> 14;
    let a1_l_q28 = a_q28[1].wrapping_neg() & 0x0000_3FFF;
    let a1_u_q28 = a_q28[1].wrapping_neg() >> 14;
    for v in x.iter_mut() {
        let inval = i32::from(*v);
        let out32_q14 = lshift(smlawb(s[0], b_q28[0], inval), 2);
        s[0] = s[1].wrapping_add(rshift_round(smulwb(out32_q14, a0_l_q28), 14));
        s[0] = smlawb(s[0], out32_q14, a0_u_q28);
        s[0] = smlawb(s[0], b_q28[1], inval);
        s[1] = rshift_round(smulwb(out32_q14, a1_l_q28), 14);
        s[1] = smlawb(s[1], out32_q14, a1_u_q28);
        s[1] = smlawb(s[1], b_q28[2], inval);
        *v = sat16(out32_q14.wrapping_add((1 << 14) - 1) >> 14);
    }
}

/// `silk_LP_variable_cutoff`: filters `frame` in place while a transition runs (`mode` not 0).
pub(super) fn lp_variable_cutoff(lp: &mut LpState, frame: &mut [i16]) {
    if lp.mode != 0 {
        // TRANSITION_INT_STEPS is 64.
        let fac = lshift(TRANSITION_FRAMES - lp.transition_frame_no, 16 - 6);
        let ind = (fac >> 16) as usize;
        let fac_q16 = fac - lshift(ind as i32, 16);
        let (b, a) = interpolate_filter_taps(ind, fac_q16);
        lp.transition_frame_no = (lp.transition_frame_no + lp.mode).clamp(0, TRANSITION_FRAMES);
        biquad_alt_stride1(frame, &b, &a, &mut lp.in_lp_state);
    }
}
