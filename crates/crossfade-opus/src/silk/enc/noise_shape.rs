//! Noise shaping analysis (libopus `silk/fixed/noise_shape_analysis_FIX.c`): the
//! quantisation gains before rate control, the AR shaping filters, the low-frequency,
//! tilt and harmonic shaping, and the quantiser offset of unvoiced frames.

use super::fix::{
    add_pos_sat32, div32_16, div32_varq, fix_f32, fix_f64, inverse32_varq, lin2log, log2lin,
    lshift, lshift_sat32, rshift_round, sat16, sigm_q15, smlawb, smlaww, smulbb, smulwb, smulww,
    sqrt_approx, sum_sqr_shift,
};
use super::sigproc::{
    MAX_SHAPE_LPC_ORDER, apply_sine_window, autocorr, k2a_q16, schur64, warped_autocorrelation,
};
use super::state::{ChannelEncoder, EncoderControl};
use crate::silk::nlsf::{bwexpander_32, lpc_fit};
use crate::silk::state::{MAX_NB_SUBFR, SUB_FRAME_LENGTH_MS, TYPE_VOICED};

const BG_SNR_DECR_DB: f32 = 2.0;
const HARM_SNR_INCR_DB: f32 = 2.0;
const ENERGY_VARIATION_THRESHOLD_QNT_OFFSET: f32 = 0.6;
const FIND_PITCH_WHITE_NOISE_FRACTION: f32 = 1e-3;
const BANDWIDTH_EXPANSION: f32 = 0.94;
const SHAPE_WHITE_NOISE_FRACTION: f32 = 3e-5;
const HARMONIC_SHAPING: f32 = 0.3;
const HIGH_RATE_OR_LOW_QUALITY_HARMONIC_SHAPING: f32 = 0.2;
const HP_NOISE_COEF: f32 = 0.25;
const HARM_HP_NOISE_COEF: f32 = 0.35;
const LOW_FREQ_SHAPING: f32 = 4.0;
const LOW_QUALITY_LOW_FREQ_SHAPING_DECR: f32 = 0.5;
const SUBFR_SMTH_COEF: f32 = 0.4;
const MIN_QGAIN_DB: f64 = 2.0;

/// `warped_gain`: the gain making warped coefficients' log response zero-mean, in Q16.
fn warped_gain(coefs_q24: &[i32], lambda_q16: i32, order: usize) -> i32 {
    let lambda_q16 = -lambda_q16;
    let mut gain_q24 = coefs_q24[order - 1];
    for i in (0..order - 1).rev() {
        gain_q24 = smlawb(coefs_q24[i], gain_q24, lambda_q16);
    }
    gain_q24 = smlawb(fix_f64(1.0, 24), gain_q24, -lambda_q16);
    inverse32_varq(gain_q24, 40)
}

/// `limit_warped_coefs`: to monic pseudo-warped coefficients, chirped until their largest
/// is within `limit_q24`.
fn limit_warped_coefs(coefs_q24: &mut [i32], lambda_q16: i32, limit_q24: i32, order: usize) {
    let to_monic = |c: &mut [i32]| {
        for i in (1..order).rev() {
            c[i - 1] = smlawb(c[i - 1], c[i], -lambda_q16);
        }
        let nom_q16 = smlawb(fix_f64(1.0, 16), -lambda_q16, lambda_q16);
        let den_q24 = smlawb(fix_f64(1.0, 24), c[0], lambda_q16);
        let gain_q16 = div32_varq(nom_q16, den_q24, 24);
        for v in c[..order].iter_mut() {
            *v = smulww(gain_q16, *v);
        }
        gain_q16
    };
    let mut gain_q16 = to_monic(coefs_q24);
    let limit_q20 = limit_q24 >> 4;
    for iter in 0..10 {
        let mut maxabs_q24 = -1;
        let mut ind = 0;
        for (i, &c) in coefs_q24[..order].iter().enumerate() {
            let tmp = c.wrapping_abs();
            if tmp > maxabs_q24 {
                maxabs_q24 = tmp;
                ind = i;
            }
        }
        let maxabs_q20 = maxabs_q24 >> 4;
        if maxabs_q20 <= limit_q20 {
            return;
        }
        for i in 1..order {
            coefs_q24[i - 1] = smlawb(coefs_q24[i - 1], coefs_q24[i], lambda_q16);
        }
        gain_q16 = inverse32_varq(gain_q16, 32);
        for v in coefs_q24[..order].iter_mut() {
            *v = smulww(gain_q16, *v);
        }
        let chirp_q16 = fix_f64(0.99, 16)
            - div32_varq(
                smulwb(
                    maxabs_q20 - limit_q20,
                    super::fix::smlabb(fix_f64(0.8, 10), fix_f64(0.1, 10), iter),
                ),
                maxabs_q20.wrapping_mul(ind as i32 + 1),
                22,
            );
        bwexpander_32(&mut coefs_q24[..order], chirp_q16);
        gain_q16 = to_monic(coefs_q24);
    }
}

/// `silk_noise_shape_analysis_FIX`. `pitch_res` is the pitch analysis residual at the
/// frame; `x` the input buffer and `x0` the frame's first sample in it (`la_shape` before
/// it are read).
pub(super) fn noise_shape_analysis(
    st: &mut ChannelEncoder,
    ctrl: &mut EncoderControl,
    pitch_res: &[i16],
    x: &[i16],
    x0: usize,
) {
    let mut snr_adj_db_q7 = st.snr_db_q7;
    ctrl.input_quality_q14 = (st.input_quality_bands_q15[0] + st.input_quality_bands_q15[1]) >> 2;
    ctrl.coding_quality_q14 = sigm_q15(rshift_round(snr_adj_db_q7 - fix_f64(20.0, 7), 4)) >> 1;

    if !st.use_cbr {
        let b_q8 = fix_f64(1.0, 8) - st.speech_activity_q8;
        let b_q8 = smulwb(lshift(b_q8, 8), b_q8);
        snr_adj_db_q7 = smlawb(
            snr_adj_db_q7,
            smulbb(fix_f32(-BG_SNR_DECR_DB, 7) >> (4 + 1), b_q8),
            smulwb(
                fix_f64(1.0, 14) + ctrl.input_quality_q14,
                ctrl.coding_quality_q14,
            ),
        );
    }
    if i32::from(st.indices.signal_type) == TYPE_VOICED {
        snr_adj_db_q7 = smlawb(snr_adj_db_q7, fix_f32(HARM_SNR_INCR_DB, 8), st.ltp_corr_q15);
    } else {
        snr_adj_db_q7 = smlawb(
            snr_adj_db_q7,
            smlawb(fix_f64(6.0, 9), -fix_f64(0.4, 18), st.snr_db_q7),
            fix_f64(1.0, 14) - ctrl.input_quality_q14,
        );
    }

    // Sparseness: the quantiser offset of unvoiced frames.
    if i32::from(st.indices.signal_type) == TYPE_VOICED {
        st.indices.quant_offset_type = 0;
    } else {
        let n_samples = (st.fs_khz as usize) << 1;
        let mut energy_variation_q7 = 0;
        let mut log_energy_prev_q7 = 0;
        let n_segs = smulbb(SUB_FRAME_LENGTH_MS as i32, st.nb_subfr as i32) / 2;
        for k in 0..n_segs as usize {
            let (mut nrg, scale) = sum_sqr_shift(&pitch_res[k * n_samples..][..n_samples]);
            nrg += n_samples as i32 >> scale;
            let log_energy_q7 = lin2log(nrg);
            if k > 0 {
                energy_variation_q7 += (log_energy_q7 - log_energy_prev_q7).abs();
            }
            log_energy_prev_q7 = log_energy_q7;
        }
        st.indices.quant_offset_type = if energy_variation_q7
            > fix_f32(ENERGY_VARIATION_THRESHOLD_QNT_OFFSET, 7) * (n_segs - 1)
        {
            0
        } else {
            1
        };
    }

    let strength_q16 = smulwb(
        ctrl.pred_gain_q16,
        fix_f32(FIND_PITCH_WHITE_NOISE_FRACTION, 16),
    );
    let bw_exp_q16 = div32_varq(
        fix_f32(BANDWIDTH_EXPANSION, 16),
        smlaww(fix_f64(1.0, 16), strength_q16, strength_q16),
        16,
    );
    let warping_q16 = if st.warping_q16 > 0 {
        smlawb(st.warping_q16, ctrl.coding_quality_q14, fix_f64(0.01, 18))
    } else {
        0
    };

    let order = st.shaping_lpc_order;
    let mut x_windowed = [0i16; 15 * 16];
    let mut xp = x0 - st.la_shape;
    for k in 0..st.nb_subfr {
        let flat_part = st.fs_khz as usize * 3;
        let slope_part = (st.shape_win_length - flat_part) >> 1;
        apply_sine_window(&mut x_windowed, &x[xp..], 1, slope_part);
        let mut shift = slope_part;
        x_windowed[shift..shift + flat_part]
            .copy_from_slice(&x[xp + shift..xp + shift + flat_part]);
        shift += flat_part;
        apply_sine_window(&mut x_windowed[shift..], &x[xp + shift..], 2, slope_part);
        xp += st.subfr_length;

        let win = &x_windowed[..st.shape_win_length];
        let mut auto_corr = [0i32; MAX_SHAPE_LPC_ORDER + 1];
        let scale = if st.warping_q16 > 0 {
            warped_autocorrelation(&mut auto_corr, win, warping_q16, order)
        } else {
            autocorr(&mut auto_corr[..order + 1], win)
        };
        auto_corr[0] = auto_corr[0].wrapping_add(
            smulwb(auto_corr[0] >> 4, fix_f32(SHAPE_WHITE_NOISE_FRACTION, 20)).max(1),
        );
        let mut refl_coef_q16 = [0i32; MAX_SHAPE_LPC_ORDER];
        let mut nrg = schur64(&mut refl_coef_q16, &auto_corr, order);
        let mut ar_q24 = [0i32; MAX_SHAPE_LPC_ORDER];
        k2a_q16(&mut ar_q24, &refl_coef_q16[..order]);

        let mut qnrg = -scale;
        if qnrg & 1 != 0 {
            qnrg -= 1;
            nrg >>= 1;
        }
        let tmp32 = sqrt_approx(nrg);
        qnrg >>= 1;
        ctrl.gains_q16[k] = lshift_sat32(tmp32, (16 - qnrg) as u32);

        if st.warping_q16 > 0 {
            let gain_mult_q16 = warped_gain(&ar_q24, warping_q16, order);
            if ctrl.gains_q16[k] < fix_f64(0.25, 16) {
                ctrl.gains_q16[k] = smulww(ctrl.gains_q16[k], gain_mult_q16);
            } else {
                ctrl.gains_q16[k] = smulww(rshift_round(ctrl.gains_q16[k], 1), gain_mult_q16);
                ctrl.gains_q16[k] = if ctrl.gains_q16[k] >= (i32::MAX >> 1) {
                    i32::MAX
                } else {
                    lshift(ctrl.gains_q16[k], 1)
                };
            }
        }

        bwexpander_32(&mut ar_q24[..order], bw_exp_q16);
        let ar = &mut ctrl.ar_q13[k * MAX_SHAPE_LPC_ORDER..][..MAX_SHAPE_LPC_ORDER];
        if st.warping_q16 > 0 {
            limit_warped_coefs(&mut ar_q24, warping_q16, fix_f64(3.999, 24), order);
            for i in 0..order {
                ar[i] = sat16(rshift_round(ar_q24[i], 11));
            }
        } else {
            lpc_fit(&mut ar[..order], &mut ar_q24[..order], 13, 24);
        }
    }

    // Higher gains during low speech activity, and a floor.
    let gain_mult_q16 = log2lin(-smlawb(-fix_f64(16.0, 7), snr_adj_db_q7, fix_f64(0.16, 16)));
    let gain_add_q16 = log2lin(smlawb(
        fix_f64(16.0, 7),
        fix_f64(MIN_QGAIN_DB, 7),
        fix_f64(0.16, 16),
    ));
    for g in ctrl.gains_q16[..st.nb_subfr].iter_mut() {
        *g = smulww(*g, gain_mult_q16);
        *g = add_pos_sat32(*g, gain_add_q16);
    }

    // Low-frequency shaping and noise tilt; less for noisy input.
    let mut strength_q16 = fix_f32(LOW_FREQ_SHAPING, 4).wrapping_mul(smlawb(
        fix_f64(1.0, 12),
        fix_f32(LOW_QUALITY_LOW_FREQ_SHAPING_DECR, 13),
        st.input_quality_bands_q15[0] - fix_f64(1.0, 15),
    ));
    strength_q16 = strength_q16.wrapping_mul(st.speech_activity_q8) >> 8;
    let tilt_q16 = if i32::from(st.indices.signal_type) == TYPE_VOICED {
        let fs_khz_inv = div32_16(fix_f64(0.2, 14), st.fs_khz);
        for k in 0..st.nb_subfr {
            let b_q14 = fs_khz_inv + div32_16(fix_f64(3.0, 14), ctrl.pitch_l[k]);
            ctrl.lf_shp_q14[k] = lshift(fix_f64(1.0, 14) - b_q14 - smulwb(strength_q16, b_q14), 16);
            ctrl.lf_shp_q14[k] |= i32::from((b_q14 - fix_f64(1.0, 14)) as u16);
        }
        -fix_f32(HP_NOISE_COEF, 16)
            - smulwb(
                fix_f64(1.0, 16) - fix_f32(HP_NOISE_COEF, 16),
                smulwb(fix_f32(HARM_HP_NOISE_COEF, 24), st.speech_activity_q8),
            )
    } else {
        let b_q14 = div32_16(21299, st.fs_khz);
        ctrl.lf_shp_q14[0] = lshift(
            fix_f64(1.0, 14) - b_q14 - smulwb(strength_q16, smulwb(fix_f64(0.6, 16), b_q14)),
            16,
        );
        ctrl.lf_shp_q14[0] |= i32::from((b_q14 - fix_f64(1.0, 14)) as u16);
        for k in 1..st.nb_subfr {
            ctrl.lf_shp_q14[k] = ctrl.lf_shp_q14[0];
        }
        -fix_f32(HP_NOISE_COEF, 16)
    };

    let harm_shape_gain_q16 = if i32::from(st.indices.signal_type) == TYPE_VOICED {
        let g = smlawb(
            fix_f32(HARMONIC_SHAPING, 16),
            fix_f64(1.0, 16)
                - smulwb(
                    fix_f64(1.0, 18) - lshift(ctrl.coding_quality_q14, 4),
                    ctrl.input_quality_q14,
                ),
            fix_f32(HIGH_RATE_OR_LOW_QUALITY_HARMONIC_SHAPING, 16),
        );
        smulwb(lshift(g, 1), sqrt_approx(lshift(st.ltp_corr_q15, 15)))
    } else {
        0
    };

    let ps = &mut st.s_shape;
    for k in 0..MAX_NB_SUBFR {
        ps.harm_shape_gain_smth_q16 = smlawb(
            ps.harm_shape_gain_smth_q16,
            harm_shape_gain_q16 - ps.harm_shape_gain_smth_q16,
            fix_f32(SUBFR_SMTH_COEF, 16),
        );
        ps.tilt_smth_q16 = smlawb(
            ps.tilt_smth_q16,
            tilt_q16 - ps.tilt_smth_q16,
            fix_f32(SUBFR_SMTH_COEF, 16),
        );
        ctrl.harm_shape_gain_q14[k] = rshift_round(ps.harm_shape_gain_smth_q16, 2);
        ctrl.tilt_q14[k] = rshift_round(ps.tilt_smth_q16, 2);
    }
}
