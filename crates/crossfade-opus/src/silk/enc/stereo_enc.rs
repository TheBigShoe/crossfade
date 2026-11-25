//! Stereo, encoder side (libopus `silk/stereo_LR_to_MS.c`, `stereo_find_predictor.c`,
//! `stereo_quant_pred.c`, `stereo_encode_pred.c`): left/right to an adaptive mid/side pair,
//! the side predicted from the mid in two bands, and the width reduced at low rates.

use super::fix::{
    add_lshift32, div32_varq, fix_f64, inner_prod_aligned_scale, lshift, rshift_round, sat16,
    smlabb, smlawb, smulbb, smulwb, sqrt_approx, sub_lshift32, sum_sqr_shift,
};
use super::state::LA_SHAPE_MS;
use crate::range_enc::RangeEncoder;
use crate::silk::state::{MAX_FRAME_LENGTH, MAX_FRAMES_PER_PACKET};
use crate::silk::tables::{
    STEREO_ONLY_CODE_MID_ICDF, STEREO_PRED_JOINT_ICDF, STEREO_PRED_QUANT_Q13, UNIFORM3_ICDF,
    UNIFORM5_ICDF,
};

const STEREO_INTERP_LEN_MS: usize = 8;
const STEREO_QUANT_SUB_STEPS: i32 = 5;
const STEREO_RATIO_SMOOTH_COEF: f64 = 0.01;

/// `stereo_enc_state`.
#[derive(Debug, Clone, Default)]
pub(super) struct StereoEncState {
    pub(super) pred_prev_q13: [i16; 2],
    pub(super) s_mid: [i16; 2],
    pub(super) s_side: [i16; 2],
    pub(super) mid_side_amp_q0: [i32; 4],
    pub(super) smth_width_q14: i16,
    pub(super) width_prev_q14: i16,
    pub(super) silent_side_len: i16,
    pub(super) pred_ix: [[[i8; 3]; 2]; MAX_FRAMES_PER_PACKET],
    pub(super) mid_only_flags: [bool; MAX_FRAMES_PER_PACKET],
}

/// `silk_stereo_find_predictor`: the least-squares predictor of `y` from `x` in Q13, and the
/// ratio of the smoothed residual and mid norms in Q14.
fn find_predictor(
    x: &[i16],
    y: &[i16],
    mid_res_amp_q0: &mut [i32],
    smooth_coef_q16: i32,
) -> (i32, i32) {
    let (nrgx, scale1) = sum_sqr_shift(x);
    let (nrgy, scale2) = sum_sqr_shift(y);
    let mut scale = scale1.max(scale2);
    scale += scale & 1;
    let mut nrgy = nrgy >> (scale - scale2);
    let nrgx = (nrgx >> (scale - scale1)).max(1);
    let corr = inner_prod_aligned_scale(x, y, scale as u32);
    let pred_q13 = div32_varq(corr, nrgx, 13).clamp(-(1 << 14), 1 << 14);
    let pred2_q10 = smulwb(pred_q13, pred_q13);
    let smooth_coef_q16 = smooth_coef_q16.max(pred2_q10.abs());
    let scale = scale >> 1;
    mid_res_amp_q0[0] = smlawb(
        mid_res_amp_q0[0],
        lshift(sqrt_approx(nrgx), scale as u32) - mid_res_amp_q0[0],
        smooth_coef_q16,
    );
    nrgy = sub_lshift32(nrgy, smulwb(corr, pred_q13), 3 + 1);
    nrgy = add_lshift32(nrgy, smulwb(nrgx, pred2_q10), 6);
    mid_res_amp_q0[1] = smlawb(
        mid_res_amp_q0[1],
        lshift(sqrt_approx(nrgy), scale as u32) - mid_res_amp_q0[1],
        smooth_coef_q16,
    );
    let ratio_q14 = div32_varq(mid_res_amp_q0[1], mid_res_amp_q0[0].max(1), 14).clamp(0, 32767);
    (pred_q13, ratio_q14)
}

/// `silk_stereo_quant_pred`: the two predictors quantised (in place) and their indices.
fn quant_pred(pred_q13: &mut [i32; 2], ix: &mut [[i8; 3]; 2]) {
    let step_frac_q16 = fix_f64(0.5 / f64::from(STEREO_QUANT_SUB_STEPS), 16);
    for n in 0..2 {
        let mut err_min_q13 = i32::MAX;
        let mut quant_pred_q13 = 0;
        'search: for i in 0..STEREO_PRED_QUANT_Q13.len() - 1 {
            let low_q13 = i32::from(STEREO_PRED_QUANT_Q13[i]);
            let step_q13 = smulwb(
                i32::from(STEREO_PRED_QUANT_Q13[i + 1]) - low_q13,
                step_frac_q16,
            );
            for j in 0..STEREO_QUANT_SUB_STEPS {
                let lvl_q13 = smlabb(low_q13, step_q13, 2 * j + 1);
                let err_q13 = (pred_q13[n] - lvl_q13).abs();
                if err_q13 < err_min_q13 {
                    err_min_q13 = err_q13;
                    quant_pred_q13 = lvl_q13;
                    ix[n][0] = i as i8;
                    ix[n][1] = j as i8;
                } else {
                    // Past the optimum.
                    break 'search;
                }
            }
        }
        ix[n][2] = ix[n][0] / 3;
        ix[n][0] -= ix[n][2] * 3;
        pred_q13[n] = quant_pred_q13;
    }
    pred_q13[0] -= pred_q13[1];
}

/// `silk_stereo_encode_pred`.
pub(super) fn encode_pred(enc: &mut RangeEncoder, ix: &[[i8; 3]; 2]) {
    let n = 5 * ix[0][2] + ix[1][2];
    enc.enc_icdf(n as usize, &STEREO_PRED_JOINT_ICDF, 8);
    for v in ix {
        enc.enc_icdf(v[0] as usize, &UNIFORM3_ICDF, 8);
        enc.enc_icdf(v[1] as usize, &UNIFORM5_ICDF, 8);
    }
}

/// `silk_stereo_encode_mid_only`.
pub(super) fn encode_mid_only(enc: &mut RangeEncoder, mid_only: bool) {
    enc.enc_icdf(usize::from(mid_only), &STEREO_ONLY_CODE_MID_ICDF, 8);
}

/// What `silk_stereo_LR_to_MS` decides for a frame.
pub(super) struct MsChoice {
    pub(super) mid_side_rates_bps: [i32; 2],
}

/// `silk_stereo_LR_to_MS`: `buf1` and `buf2` are the two channels' input buffers (two
/// samples of history, then the frame); `buf1` becomes the mid and `buf2` the side residual.
/// Writes the frame's predictor indices and mid-only flag into the state at `frame`.
#[allow(clippy::too_many_arguments)]
pub(super) fn lr_to_ms(
    state: &mut StereoEncState,
    buf1: &mut [i16; MAX_FRAME_LENGTH + 2],
    buf2: &mut [i16; MAX_FRAME_LENGTH + 2],
    frame: usize,
    total_rate_bps: i32,
    prev_speech_act_q8: i32,
    to_mono: bool,
    fs_khz: usize,
    frame_length: usize,
) -> MsChoice {
    let mut side = [0i16; MAX_FRAME_LENGTH + 2];
    for n in 0..frame_length + 2 {
        let sum = i32::from(buf1[n]) + i32::from(buf2[n]);
        let diff = i32::from(buf1[n]) - i32::from(buf2[n]);
        buf1[n] = rshift_round(sum, 1) as i16;
        side[n] = sat16(rshift_round(diff, 1));
    }
    let mid = buf1;
    mid[..2].copy_from_slice(&state.s_mid);
    side[..2].copy_from_slice(&state.s_side);
    state
        .s_mid
        .copy_from_slice(&mid[frame_length..frame_length + 2]);
    state
        .s_side
        .copy_from_slice(&side[frame_length..frame_length + 2]);

    let mut lp_mid = [0i16; MAX_FRAME_LENGTH];
    let mut hp_mid = [0i16; MAX_FRAME_LENGTH];
    let mut lp_side = [0i16; MAX_FRAME_LENGTH];
    let mut hp_side = [0i16; MAX_FRAME_LENGTH];
    let three_tap = |s: &[i16], n: usize| {
        rshift_round(
            add_lshift32(i32::from(s[n]) + i32::from(s[n + 2]), s[n + 1].into(), 1),
            2,
        )
    };
    for n in 0..frame_length {
        let sum = three_tap(&mid[..], n);
        lp_mid[n] = sum as i16;
        hp_mid[n] = (i32::from(mid[n + 1]) - sum) as i16;
        let sum = three_tap(&side, n);
        lp_side[n] = sum as i16;
        hp_side[n] = (i32::from(side[n + 1]) - sum) as i16;
    }

    let is_10ms = frame_length == 10 * fs_khz;
    let smooth_coef_q16 = if is_10ms {
        fix_f64(STEREO_RATIO_SMOOTH_COEF / 2.0, 16)
    } else {
        fix_f64(STEREO_RATIO_SMOOTH_COEF, 16)
    };
    let smooth_coef_q16 = smulwb(
        smulbb(prev_speech_act_q8, prev_speech_act_q8),
        smooth_coef_q16,
    );
    let (p0, lp_ratio_q14) = find_predictor(
        &lp_mid[..frame_length],
        &lp_side[..frame_length],
        &mut state.mid_side_amp_q0[0..2],
        smooth_coef_q16,
    );
    let (p1, hp_ratio_q14) = find_predictor(
        &hp_mid[..frame_length],
        &hp_side[..frame_length],
        &mut state.mid_side_amp_q0[2..4],
        smooth_coef_q16,
    );
    let mut pred_q13 = [p0, p1];
    let frac_q16 = smlabb(hp_ratio_q14, lp_ratio_q14, 3).min(fix_f64(1.0, 16));

    let mut total_rate_bps = (total_rate_bps - if is_10ms { 1200 } else { 600 }).max(1);
    let min_mid_rate_bps = smlabb(2000, fs_khz as i32, 600);
    let frac_3_q16 = 3 * frac_q16;
    let mut rates = [0i32; 2];
    rates[0] = div32_varq(total_rate_bps, fix_f64(13.0, 16) + frac_3_q16, 16 + 3);
    let mut width_q14;
    if rates[0] < min_mid_rate_bps {
        rates[0] = min_mid_rate_bps;
        rates[1] = total_rate_bps - rates[0];
        width_q14 = div32_varq(
            lshift(rates[1], 1) - min_mid_rate_bps,
            smulwb(fix_f64(1.0, 16) + frac_3_q16, min_mid_rate_bps),
            14 + 2,
        )
        .clamp(0, fix_f64(1.0, 14));
    } else {
        rates[1] = total_rate_bps - rates[0];
        width_q14 = fix_f64(1.0, 14);
    }
    state.smth_width_q14 = smlawb(
        state.smth_width_q14.into(),
        width_q14 - i32::from(state.smth_width_q14),
        smooth_coef_q16,
    ) as i16;

    let ix = &mut state.pred_ix[frame];
    let mut mid_only = false;
    let smth = i32::from(state.smth_width_q14);
    let scale_down = |p: &mut [i32; 2]| {
        p[0] = smulbb(smth, p[0]) >> 14;
        p[1] = smulbb(smth, p[1]) >> 14;
    };
    if to_mono {
        width_q14 = 0;
        pred_q13 = [0, 0];
        quant_pred(&mut pred_q13, ix);
    } else if state.width_prev_q14 == 0
        && (8 * total_rate_bps < 13 * min_mid_rate_bps
            || smulwb(frac_q16, smth) < fix_f64(0.05, 14))
    {
        // Panned mono; the previous frame already had zero width.
        scale_down(&mut pred_q13);
        quant_pred(&mut pred_q13, ix);
        width_q14 = 0;
        pred_q13 = [0, 0];
        rates = [total_rate_bps, 0];
        mid_only = true;
    } else if state.width_prev_q14 != 0
        && (8 * total_rate_bps < 11 * min_mid_rate_bps
            || smulwb(frac_q16, smth) < fix_f64(0.02, 14))
    {
        // Towards zero width.
        scale_down(&mut pred_q13);
        quant_pred(&mut pred_q13, ix);
        width_q14 = 0;
        pred_q13 = [0, 0];
    } else if smth > fix_f64(0.95, 14) {
        quant_pred(&mut pred_q13, ix);
        width_q14 = fix_f64(1.0, 14);
    } else {
        scale_down(&mut pred_q13);
        quant_pred(&mut pred_q13, ix);
        width_q14 = smth;
    }

    // Keep coding the side until the tapered output has been sent.
    if mid_only {
        state.silent_side_len = state
            .silent_side_len
            .wrapping_add((frame_length - STEREO_INTERP_LEN_MS * fs_khz) as i16);
        if i32::from(state.silent_side_len) < (LA_SHAPE_MS * fs_khz) as i32 {
            mid_only = false;
        } else {
            state.silent_side_len = 10000;
        }
    } else {
        state.silent_side_len = 0;
    }
    state.mid_only_flags[frame] = mid_only;
    total_rate_bps = total_rate_bps.max(1);
    if !mid_only && rates[1] < 1 {
        rates[1] = 1;
        rates[0] = (total_rate_bps - rates[1]).max(1);
    }

    // Interpolate the predictors and take the prediction from the side.
    let interp = STEREO_INTERP_LEN_MS * fs_khz;
    let mut pred0_q13 = -i32::from(state.pred_prev_q13[0]);
    let mut pred1_q13 = -i32::from(state.pred_prev_q13[1]);
    let mut w_q24 = lshift(state.width_prev_q14.into(), 10);
    let denom_q16 = (1 << 16) / interp as i32;
    let delta0_q13 = -rshift_round(
        smulbb(pred_q13[0] - i32::from(state.pred_prev_q13[0]), denom_q16),
        16,
    );
    let delta1_q13 = -rshift_round(
        smulbb(pred_q13[1] - i32::from(state.pred_prev_q13[1]), denom_q16),
        16,
    );
    let deltaw_q24 = lshift(
        smulwb(width_q14 - i32::from(state.width_prev_q14), denom_q16),
        10,
    );
    for n in 0..frame_length {
        if n < interp {
            pred0_q13 += delta0_q13;
            pred1_q13 += delta1_q13;
            w_q24 += deltaw_q24;
        } else if n == interp {
            pred0_q13 = -pred_q13[0];
            pred1_q13 = -pred_q13[1];
            w_q24 = lshift(width_q14, 10);
        }
        let sum = lshift(
            add_lshift32(
                i32::from(mid[n]) + i32::from(mid[n + 2]),
                mid[n + 1].into(),
                1,
            ),
            9,
        );
        let sum = smlawb(smulwb(w_q24, side[n + 1].into()), sum, pred0_q13);
        let sum = smlawb(sum, lshift(mid[n + 1].into(), 11), pred1_q13);
        buf2[n + 1] = sat16(rshift_round(sum, 8));
    }
    state.pred_prev_q13 = [pred_q13[0] as i16, pred_q13[1] as i16];
    state.width_prev_q14 = width_q14 as i16;
    MsChoice {
        mid_side_rates_bps: rates,
    }
}
