//! Stereo prediction (libopus `silk/stereo_decode_pred.c`, `stereo_MS_to_LR.c`): the mid/side
//! predictor weights from the bitstream and the mid/side to left/right conversion, with the
//! predictor interpolated over the first 8 ms and the one-sample delay the encoder applies.

use super::macros::{rshift_round, sat16, smlabb, smlawb, smulbb, smulwb};
use super::tables::{
    STEREO_ONLY_CODE_MID_ICDF, STEREO_PRED_JOINT_ICDF, STEREO_PRED_QUANT_Q13, UNIFORM3_ICDF,
    UNIFORM5_ICDF,
};
use crate::range::RangeDecoder;

const STEREO_INTERP_LEN_MS: usize = 8;

/// `stereo_dec_state`.
#[derive(Debug, Clone, Default)]
pub(super) struct StereoState {
    pub(super) pred_prev_q13: [i16; 2],
    pub(super) s_mid: [i16; 2],
    pub(super) s_side: [i16; 2],
}

/// `silk_stereo_decode_pred`.
pub(super) fn decode_pred(dec: &mut RangeDecoder<'_>) -> [i32; 2] {
    let n = dec.dec_icdf(&STEREO_PRED_JOINT_ICDF, 8) as i32;
    let mut ix = [[0i32; 3]; 2];
    ix[0][2] = n / 5;
    ix[1][2] = n - 5 * ix[0][2];
    for row in &mut ix {
        row[0] = dec.dec_icdf(&UNIFORM3_ICDF, 8) as i32;
        row[1] = dec.dec_icdf(&UNIFORM5_ICDF, 8) as i32;
    }
    let mut pred = [0i32; 2];
    for (p, row) in pred.iter_mut().zip(&mut ix) {
        row[0] += 3 * row[2];
        // At most 3 * 4 + 2 = 14, below the table's 16 entries.
        let low = i32::from(STEREO_PRED_QUANT_Q13[row[0] as usize]);
        let step = smulwb(
            i32::from(STEREO_PRED_QUANT_Q13[row[0] as usize + 1]) - low,
            6554, // SILK_FIX_CONST(0.5 / STEREO_QUANT_SUB_STEPS, 16)
        );
        *p = smlabb(low, step, 2 * row[1] + 1);
    }
    pred[0] -= pred[1];
    pred
}

/// `silk_stereo_decode_mid_only`.
pub(super) fn decode_mid_only(dec: &mut RangeDecoder<'_>) -> bool {
    dec.dec_icdf(&STEREO_ONLY_CODE_MID_ICDF, 8) != 0
}

/// `silk_stereo_MS_to_LR`: `x1` (mid) and `x2` (side) each hold `frame_length + 2` samples,
/// the frame at offset 2; they become left and right, delayed by one sample.
pub(super) fn ms_to_lr(
    state: &mut StereoState,
    x1: &mut [i16],
    x2: &mut [i16],
    pred_q13: [i32; 2],
    fs_khz: usize,
    frame_length: usize,
) {
    x1[..2].copy_from_slice(&state.s_mid);
    x2[..2].copy_from_slice(&state.s_side);
    state
        .s_mid
        .copy_from_slice(&x1[frame_length..frame_length + 2]);
    state
        .s_side
        .copy_from_slice(&x2[frame_length..frame_length + 2]);

    let mut pred0 = i32::from(state.pred_prev_q13[0]);
    let mut pred1 = i32::from(state.pred_prev_q13[1]);
    let interp = (STEREO_INTERP_LEN_MS * fs_khz).min(frame_length);
    let denom_q16 = (1 << 16) / (STEREO_INTERP_LEN_MS * fs_khz).max(1) as i32;
    let delta0 = rshift_round(smulbb(pred_q13[0] - pred0, denom_q16), 16);
    let delta1 = rshift_round(smulbb(pred_q13[1] - pred1, denom_q16), 16);
    let side = |x1: &[i16], x2: &[i16], n: usize, p0: i32, p1: i32| {
        let mid = i32::from(x1[n]) + i32::from(x1[n + 2]) + (i32::from(x1[n + 1]) << 1);
        let sum = mid << 9;
        let sum = smlawb(i32::from(x2[n + 1]) << 8, sum, p0);
        let sum = smlawb(sum, i32::from(x1[n + 1]) << 11, p1);
        sat16(rshift_round(sum, 8))
    };
    for n in 0..interp {
        pred0 += delta0;
        pred1 += delta1;
        x2[n + 1] = side(x1, x2, n, pred0, pred1);
    }
    for n in interp..frame_length {
        x2[n + 1] = side(x1, x2, n, pred_q13[0], pred_q13[1]);
    }
    state.pred_prev_q13 = [pred_q13[0] as i16, pred_q13[1] as i16];

    for n in 1..=frame_length {
        let (m, s) = (i32::from(x1[n]), i32::from(x2[n]));
        x1[n] = sat16(m + s);
        x2[n] = sat16(m - s);
    }
}
