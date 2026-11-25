//! Voice activity detection (libopus `silk/VAD.c`, `ana_filt_bank_1.c`): four subbands'
//! energies against their tracked noise levels give the speech activity, the input tilt and
//! each band's quality.

use super::fix::{
    add_pos_sat32, div32, lin2log, lshift, rshift_round, sat16, sigm_q15, smlabb, smlawb, smulwb,
    smulww, sqrt_approx,
};
use super::state::{ChannelEncoder, VAD_N_BANDS, VadState};
use crate::silk::state::MAX_FRAME_LENGTH;

const VAD_INTERNAL_SUBFRAMES_LOG2: usize = 2;
const VAD_INTERNAL_SUBFRAMES: usize = 1 << VAD_INTERNAL_SUBFRAMES_LOG2;
const VAD_NOISE_LEVEL_SMOOTH_COEF_Q16: i32 = 1024;
const VAD_NOISE_LEVELS_BIAS: i32 = 50;
const VAD_NEGATIVE_OFFSET_Q5: i32 = 128;
const VAD_SNR_FACTOR_Q16: i32 = 45000;
const VAD_SNR_SMOOTH_COEF_Q18: i32 = 4096;
const TILT_WEIGHTS: [i32; VAD_N_BANDS] = [30000, 6000, -12000, -12000];

/// `silk_VAD_Init`.
pub(super) fn vad_init(v: &mut VadState) {
    *v = VadState::default();
    for b in 0..VAD_N_BANDS {
        v.noise_level_bias[b] = (VAD_NOISE_LEVELS_BIAS / (b as i32 + 1)).max(1);
    }
    for b in 0..VAD_N_BANDS {
        v.nl[b] = 100 * v.noise_level_bias[b];
        v.inv_nl[b] = i32::MAX / v.nl[b];
    }
    v.counter = 15;
    v.nrg_ratio_smth_q8 = [100 * 256; VAD_N_BANDS];
}

/// `silk_ana_filt_bank_1`: splits `input` into decimated low and high bands with two
/// first-order allpass filters.
fn ana_filt_bank_1(input: &[i16], s: &mut [i32; 2], out_l: &mut [i16], out_h: &mut [i16]) {
    const A_FB1_20: i32 = 5394 << 1;
    const A_FB1_21: i32 = -24290;
    for k in 0..input.len() / 2 {
        let in32 = i32::from(input[2 * k]) << 10;
        let y = in32.wrapping_sub(s[0]);
        let x = smlawb(y, y, A_FB1_21);
        let out_1 = s[0].wrapping_add(x);
        s[0] = in32.wrapping_add(x);
        let in32 = i32::from(input[2 * k + 1]) << 10;
        let y = in32.wrapping_sub(s[1]);
        let x = smulwb(y, A_FB1_20);
        let out_2 = s[1].wrapping_add(x);
        s[1] = in32.wrapping_add(x);
        out_l[k] = sat16(rshift_round(out_2.wrapping_add(out_1), 11));
        out_h[k] = sat16(rshift_round(out_2.wrapping_sub(out_1), 11));
    }
}

/// `silk_VAD_GetSA_Q8_c`: the speech activity of `input` (one frame) into the state's
/// `speech_activity_q8`, `input_tilt_q15` and `input_quality_bands_q15`.
pub(super) fn vad_get_sa_q8(st: &mut ChannelEncoder, input: &[i16]) {
    let frame_length = st.frame_length;
    let dec1 = frame_length >> 1;
    let dec2 = frame_length >> 2;
    let dec = frame_length >> 3;
    let x_offset = [0, dec + dec2, dec + dec2 + dec, dec + dec2 + dec + dec2];
    let mut x = [0i16; MAX_FRAME_LENGTH + MAX_FRAME_LENGTH / 4];
    let v = &mut st.s_vad;

    // 0-8 kHz into 0-4 and 4-8; 0-4 into 0-2 and 2-4; 0-2 into 0-1 and 1-2.
    {
        let (lo, hi) = x.split_at_mut(x_offset[3]);
        ana_filt_bank_1(&input[..frame_length], &mut v.ana_state, lo, hi);
    }
    {
        let mut tmp = [0i16; MAX_FRAME_LENGTH / 2];
        tmp[..dec1].copy_from_slice(&x[..dec1]);
        let (lo, hi) = x.split_at_mut(x_offset[2]);
        ana_filt_bank_1(&tmp[..dec1], &mut v.ana_state1, lo, hi);
    }
    {
        let mut tmp = [0i16; MAX_FRAME_LENGTH / 4];
        tmp[..dec2].copy_from_slice(&x[..dec2]);
        let (lo, hi) = x.split_at_mut(x_offset[1]);
        ana_filt_bank_1(&tmp[..dec2], &mut v.ana_state2, lo, hi);
    }

    // A differentiator on the lowest band.
    x[dec - 1] >>= 1;
    let hp_state_tmp = x[dec - 1];
    for i in (1..dec).rev() {
        x[i - 1] >>= 1;
        x[i] = x[i].wrapping_sub(x[i - 1]);
    }
    x[0] = x[0].wrapping_sub(v.hp_state);
    v.hp_state = hp_state_tmp;

    let mut xnrg = [0i32; VAD_N_BANDS];
    for b in 0..VAD_N_BANDS {
        let dec_len = frame_length >> (VAD_N_BANDS - b).min(VAD_N_BANDS - 1);
        let sub_len = dec_len >> VAD_INTERNAL_SUBFRAMES_LOG2;
        let mut sub_off = 0;
        xnrg[b] = v.xnrg_subfr[b];
        let mut sum_squared = 0;
        for s in 0..VAD_INTERNAL_SUBFRAMES {
            sum_squared = 0i32;
            for i in 0..sub_len {
                let t = i32::from(x[x_offset[b] + i + sub_off] >> 3);
                sum_squared = smlabb(sum_squared, t, t);
            }
            xnrg[b] = if s < VAD_INTERNAL_SUBFRAMES - 1 {
                add_pos_sat32(xnrg[b], sum_squared)
            } else {
                // The look-ahead subframe counts half.
                add_pos_sat32(xnrg[b], sum_squared >> 1)
            };
            sub_off += sub_len;
        }
        v.xnrg_subfr[b] = sum_squared;
    }

    get_noise_levels(&xnrg, v);

    let mut sum_squared = 0i32;
    let mut input_tilt = 0i32;
    let mut nrg_to_noise_ratio_q8 = [0i32; VAD_N_BANDS];
    for b in 0..VAD_N_BANDS {
        let speech_nrg = xnrg[b] - v.nl[b];
        if speech_nrg > 0 {
            nrg_to_noise_ratio_q8[b] = if (xnrg[b] as u32 & 0xFF80_0000) == 0 {
                div32(lshift(xnrg[b], 8), v.nl[b] + 1)
            } else {
                div32(xnrg[b], (v.nl[b] >> 8) + 1)
            };
            let mut snr_q7 = lin2log(nrg_to_noise_ratio_q8[b]) - 8 * 128;
            sum_squared = smlabb(sum_squared, snr_q7, snr_q7);
            if speech_nrg < (1 << 20) {
                snr_q7 = smulwb(lshift(sqrt_approx(speech_nrg), 6), snr_q7);
            }
            input_tilt = smlawb(input_tilt, TILT_WEIGHTS[b], snr_q7);
        } else {
            nrg_to_noise_ratio_q8[b] = 256;
        }
    }
    sum_squared /= VAD_N_BANDS as i32;
    let p_snr_db_q7 = i32::from((3 * sqrt_approx(sum_squared)) as i16);
    let mut sa_q15 = sigm_q15(smulwb(VAD_SNR_FACTOR_Q16, p_snr_db_q7) - VAD_NEGATIVE_OFFSET_Q5);
    st.input_tilt_q15 = lshift(sigm_q15(input_tilt) - 16384, 1);

    let mut speech_nrg = 0i32;
    for b in 0..VAD_N_BANDS {
        speech_nrg = speech_nrg.wrapping_add((b as i32 + 1) * ((xnrg[b] - v.nl[b]) >> 4));
    }
    if frame_length == 20 * st.fs_khz as usize {
        speech_nrg >>= 1;
    }
    if speech_nrg <= 0 {
        sa_q15 >>= 1;
    } else if speech_nrg < 16384 {
        let s = sqrt_approx(lshift(speech_nrg, 16));
        sa_q15 = smulwb(32768 + s, sa_q15);
    }
    st.speech_activity_q8 = (sa_q15 >> 7).min(i32::from(u8::MAX));

    let mut smooth_coef_q16 = smulwb(VAD_SNR_SMOOTH_COEF_Q18, smulwb(sa_q15, sa_q15));
    if frame_length == 10 * st.fs_khz as usize {
        smooth_coef_q16 >>= 1;
    }
    for b in 0..VAD_N_BANDS {
        v.nrg_ratio_smth_q8[b] = smlawb(
            v.nrg_ratio_smth_q8[b],
            nrg_to_noise_ratio_q8[b] - v.nrg_ratio_smth_q8[b],
            smooth_coef_q16,
        );
        let snr_q7 = 3 * (lin2log(v.nrg_ratio_smth_q8[b]) - 8 * 128);
        st.input_quality_bands_q15[b] = sigm_q15((snr_q7 - 16 * 128) >> 4);
    }
}

/// `silk_VAD_GetNoiseLevels`: tracks each band's noise floor, faster at first.
fn get_noise_levels(px: &[i32; VAD_N_BANDS], v: &mut VadState) {
    let min_coef = if v.counter < 1000 {
        let c = i32::from(i16::MAX) / ((v.counter >> 4) + 1);
        v.counter += 1;
        c
    } else {
        0
    };
    for k in 0..VAD_N_BANDS {
        let nl = v.nl[k];
        let nrg = add_pos_sat32(px[k], v.noise_level_bias[k]);
        let inv_nrg = div32(i32::MAX, nrg);
        let coef = if nrg > lshift(nl, 3) {
            VAD_NOISE_LEVEL_SMOOTH_COEF_Q16 >> 3
        } else if nrg < nl {
            VAD_NOISE_LEVEL_SMOOTH_COEF_Q16
        } else {
            smulwb(smulww(inv_nrg, nl), VAD_NOISE_LEVEL_SMOOTH_COEF_Q16 << 1)
        };
        let coef = coef.max(min_coef);
        v.inv_nl[k] = smlawb(v.inv_nl[k], inv_nrg - v.inv_nl[k], coef);
        let nl = div32(i32::MAX, v.inv_nl[k]).min(0x00FF_FFFF);
        v.nl[k] = nl;
    }
}
