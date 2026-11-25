//! LPC coefficients to NLSFs and the NLSF quantiser (libopus `silk/A2NLSF.c`,
//! `NLSF_encode.c`, `NLSF_VQ.c`, `NLSF_del_dec_quant.c`, `NLSF_VQ_weights_laroia.c`,
//! `interpolate.c`; RFC 6716 4.2.7.5 read backwards).

use super::fix::{
    div32, div32_16, div32_varq, insertion_sort_increasing, lin2log, lshift, rshift_round, smlabb,
    smlaww, smulbb,
};
use crate::silk::nlsf::{bwexpander_32, nlsf_decode, nlsf_stabilize, nlsf_unpack};
use crate::silk::state::MAX_LPC_ORDER;
use crate::silk::tables_nlsf::{LSFCOSTAB_FIX_Q12, NlsfCodebook};

const NLSF_W_Q: u32 = 2;
const NLSF_QUANT_MAX_AMPLITUDE: i32 = 4;
const NLSF_QUANT_MAX_AMPLITUDE_EXT: i32 = 10;
const NLSF_QUANT_LEVEL_ADJ_Q10: i32 = 102; // SILK_FIX_CONST(0.1, 10)
const NLSF_QUANT_DEL_DEC_STATES_LOG2: usize = 2;
const NLSF_QUANT_DEL_DEC_STATES: usize = 1 << NLSF_QUANT_DEL_DEC_STATES_LOG2;
const LSF_COS_TAB_SZ_FIX: usize = 128;
const BIN_DIV_STEPS_A2NLSF_FIX: i32 = 3;
const MAX_ITERATIONS_A2NLSF_FIX: i32 = 16;

static NLSF_CB2_BITS_NB_MB_Q5: [u8; 72] = [
    255, 255, 255, 131, 6, 145, 255, 255, 255, 255, 255, 236, 93, 15, 96, 255, 255, 255, 255, 255,
    194, 83, 25, 71, 221, 255, 255, 255, 255, 162, 73, 34, 66, 162, 255, 255, 255, 210, 126, 73,
    43, 57, 173, 255, 255, 255, 201, 125, 71, 48, 58, 130, 255, 255, 255, 166, 110, 73, 57, 62,
    104, 210, 255, 255, 251, 123, 65, 55, 68, 100, 171, 255,
];

static NLSF_CB2_BITS_WB_Q5: [u8; 72] = [
    255, 255, 255, 156, 4, 154, 255, 255, 255, 255, 255, 227, 102, 15, 92, 255, 255, 255, 255, 255,
    213, 83, 24, 72, 236, 255, 255, 255, 255, 150, 76, 33, 63, 214, 255, 255, 255, 190, 121, 77,
    43, 55, 185, 255, 255, 255, 245, 137, 71, 43, 59, 139, 255, 255, 255, 255, 131, 66, 50, 66,
    107, 194, 255, 255, 166, 116, 76, 55, 53, 125, 255, 255,
];

/// The encoder's fields of `silk_NLSF_CB_struct` (`invQuantStepSize_Q6`, `ec_Rates_Q5`) for a
/// codebook, which the decoder's [`NlsfCodebook`] leaves out.
fn enc_fields(cb: &NlsfCodebook) -> (i32, &'static [u8]) {
    if cb.order == 16 {
        (427, &NLSF_CB2_BITS_WB_Q5) // SILK_FIX_CONST(1.0 / 0.15, 6)
    } else {
        (356, &NLSF_CB2_BITS_NB_MB_Q5) // SILK_FIX_CONST(1.0 / 0.18, 6)
    }
}

/// `silk_A2NLSF_trans_poly`.
fn trans_poly(p: &mut [i32], dd: usize) {
    for k in 2..=dd {
        for n in (k + 1..=dd).rev() {
            p[n - 2] = p[n - 2].wrapping_sub(p[n]);
        }
        p[k - 2] = p[k - 2].wrapping_sub(lshift(p[k], 1));
    }
}

/// `silk_A2NLSF_eval_poly`: `p` at `x` (Q12), in Q16.
fn eval_poly(p: &[i32], x: i32, dd: usize) -> i32 {
    let mut y32 = p[dd];
    let x_q16 = lshift(x, 4);
    for n in (0..dd).rev() {
        y32 = smlaww(p[n], y32, x_q16);
    }
    y32
}

/// `silk_A2NLSF_init`.
fn a2nlsf_init(a_q16: &[i32], p: &mut [i32], q: &mut [i32], dd: usize) {
    p[dd] = 1 << 16;
    q[dd] = 1 << 16;
    for k in 0..dd {
        p[k] = a_q16[dd - k - 1].wrapping_neg().wrapping_sub(a_q16[dd + k]);
        q[k] = a_q16[dd - k - 1].wrapping_neg().wrapping_add(a_q16[dd + k]);
    }
    for k in (1..=dd).rev() {
        p[k - 1] = p[k - 1].wrapping_sub(p[k]);
        q[k - 1] = q[k - 1].wrapping_add(q[k]);
    }
    trans_poly(p, dd);
    trans_poly(q, dd);
}

/// `silk_A2NLSF`: a monic whitening filter in Q16 (chirped in place until all roots are
/// found) to NLSFs in Q15.
pub(super) fn a2nlsf(nlsf: &mut [i16], a_q16: &mut [i32]) {
    let d = a_q16.len();
    let dd = d / 2;
    let mut pq = [[0i32; MAX_LPC_ORDER / 2 + 1]; 2];
    {
        let [p, q] = &mut pq;
        a2nlsf_init(a_q16, p, q, dd);
    }
    let tab = |k: usize| i32::from(LSFCOSTAB_FIX_Q12[k]);
    let mut p = 0usize;
    let mut xlo = tab(0);
    let mut ylo = eval_poly(&pq[p], xlo, dd);
    let mut root_ix;
    if ylo < 0 {
        nlsf[0] = 0;
        p = 1;
        ylo = eval_poly(&pq[p], xlo, dd);
        root_ix = 1;
    } else {
        root_ix = 0;
    }
    let mut k = 1usize;
    let mut i = 0;
    let mut thr = 0;
    loop {
        let mut xhi = tab(k);
        let mut yhi = eval_poly(&pq[p], xhi, dd);
        if (ylo <= 0 && yhi >= thr) || (ylo >= 0 && yhi <= -thr) {
            thr = i32::from(yhi == 0);
            let mut ffrac = -256;
            for m in 0..BIN_DIV_STEPS_A2NLSF_FIX {
                let xmid = rshift_round(xlo + xhi, 1);
                let ymid = eval_poly(&pq[p], xmid, dd);
                if (ylo <= 0 && ymid >= 0) || (ylo >= 0 && ymid <= 0) {
                    xhi = xmid;
                    yhi = ymid;
                } else {
                    xlo = xmid;
                    ylo = ymid;
                    ffrac += 128 >> m;
                }
            }
            if ylo.wrapping_abs() < 65536 {
                let den = ylo.wrapping_sub(yhi);
                let nom = lshift(ylo, (8 - BIN_DIV_STEPS_A2NLSF_FIX) as u32).wrapping_add(den >> 1);
                if den != 0 {
                    ffrac += div32(nom, den);
                }
            } else {
                ffrac += div32(ylo, ylo.wrapping_sub(yhi) >> (8 - BIN_DIV_STEPS_A2NLSF_FIX));
            }
            nlsf[root_ix] = ((k as i32) << 8)
                .wrapping_add(ffrac)
                .min(i32::from(i16::MAX)) as i16;
            root_ix += 1;
            if root_ix >= d {
                break;
            }
            p = root_ix & 1;
            xlo = tab(k - 1);
            ylo = lshift(1 - (root_ix as i32 & 2), 12);
        } else {
            k += 1;
            xlo = xhi;
            ylo = yhi;
            thr = 0;
            if k > LSF_COS_TAB_SZ_FIX {
                i += 1;
                if i > MAX_ITERATIONS_A2NLSF_FIX {
                    // A white spectrum.
                    nlsf[0] = ((1 << 15) / (d as i32 + 1)) as i16;
                    for k in 1..d {
                        nlsf[k] = nlsf[k - 1].wrapping_add(nlsf[0]);
                    }
                    return;
                }
                bwexpander_32(a_q16, 65536 - lshift(1, i as u32));
                {
                    let [pp, qq] = &mut pq;
                    a2nlsf_init(a_q16, pp, qq, dd);
                }
                p = 0;
                xlo = tab(0);
                ylo = eval_poly(&pq[p], xlo, dd);
                if ylo < 0 {
                    nlsf[0] = 0;
                    p = 1;
                    ylo = eval_poly(&pq[p], xlo, dd);
                    root_ix = 1;
                } else {
                    root_ix = 0;
                }
                k = 1;
            }
        }
    }
}

/// `silk_NLSF_VQ_weights_laroia`: Laroia's weights for an NLSF vector, in Q2.
pub(super) fn nlsf_vq_weights_laroia(w: &mut [i16], nlsf: &[i16]) {
    let d = nlsf.len();
    let inv = |gap: i32| div32_16(1 << (15 + NLSF_W_Q), gap.max(1));
    let n = |k: usize| i32::from(nlsf[k]);
    let mut tmp1 = inv(n(0));
    let mut tmp2 = inv(n(1) - n(0));
    w[0] = (tmp1 + tmp2).min(i32::from(i16::MAX)) as i16;
    let mut k = 1;
    while k < d - 1 {
        tmp1 = inv(n(k + 1) - n(k));
        w[k] = (tmp1 + tmp2).min(i32::from(i16::MAX)) as i16;
        tmp2 = inv(n(k + 2) - n(k + 1));
        w[k + 1] = (tmp1 + tmp2).min(i32::from(i16::MAX)) as i16;
        k += 2;
    }
    tmp1 = inv((1 << 15) - n(d - 1));
    w[d - 1] = (tmp1 + tmp2).min(i32::from(i16::MAX)) as i16;
}

/// `silk_interpolate`: `x0 + (x1 - x0) * ifact_q2 / 4`.
pub(super) fn interpolate(xi: &mut [i16], x0: &[i16], x1: &[i16], ifact_q2: i32) {
    for i in 0..xi.len() {
        let d = i32::from(x1[i]) - i32::from(x0[i]);
        xi[i] = (i32::from(x0[i]) + (smulbb(d, ifact_q2) >> 2)) as i16;
    }
}

/// `silk_NLSF_VQ`: each first-stage vector's weighted, predictive error.
fn nlsf_vq(err_q24: &mut [i32], in_q15: &[i16], cb: &NlsfCodebook) {
    let order = cb.order;
    for (i, err) in err_q24.iter_mut().enumerate().take(cb.n_vectors) {
        let cb_q8 = &cb.cb1_nlsf_q8[i * order..][..order];
        let w_q9 = &cb.cb1_wght_q9[i * order..][..order];
        let mut sum_error_q24 = 0i32;
        let mut pred_q24 = 0i32;
        let mut m = order as isize - 2;
        while m >= 0 {
            for mm in [m as usize + 1, m as usize] {
                let diff_q15 = i32::from(in_q15[mm]) - (i32::from(cb_q8[mm]) << 7);
                let diffw_q24 = smulbb(diff_q15, i32::from(w_q9[mm]));
                sum_error_q24 =
                    sum_error_q24.wrapping_add((diffw_q24 - (pred_q24 >> 1)).wrapping_abs());
                pred_q24 = diffw_q24;
            }
            m -= 2;
        }
        *err = sum_error_q24;
    }
}

/// `silk_NLSF_del_dec_quant`: the trellis quantiser of the second stage; returns the
/// rate-distortion in Q25.
#[allow(clippy::too_many_arguments)]
fn nlsf_del_dec_quant(
    indices: &mut [i8],
    x_q10: &[i16],
    w_q5: &[i16],
    pred_coef_q8: &[u8],
    ec_ix: &[i16],
    ec_rates_q5: &[u8],
    quant_step_size_q16: i32,
    inv_quant_step_size_q6: i32,
    mu_q20: i32,
    order: usize,
) -> i32 {
    const N: usize = NLSF_QUANT_DEL_DEC_STATES;
    const EXT: i32 = NLSF_QUANT_MAX_AMPLITUDE_EXT;
    let mut out0_table = [0i32; 2 * EXT as usize];
    let mut out1_table = [0i32; 2 * EXT as usize];
    for i in -EXT..EXT {
        let mut out0 = i << 10;
        let mut out1 = out0 + 1024;
        if i > 0 {
            out0 -= NLSF_QUANT_LEVEL_ADJ_Q10;
            out1 -= NLSF_QUANT_LEVEL_ADJ_Q10;
        } else if i == 0 {
            out1 -= NLSF_QUANT_LEVEL_ADJ_Q10;
        } else if i == -1 {
            out0 += NLSF_QUANT_LEVEL_ADJ_Q10;
        } else {
            out0 += NLSF_QUANT_LEVEL_ADJ_Q10;
            out1 += NLSF_QUANT_LEVEL_ADJ_Q10;
        }
        // The C keeps these in opus_int16 before multiplying.
        out0_table[(i + EXT) as usize] = smulbb(i32::from(out0 as i16), quant_step_size_q16) >> 16;
        out1_table[(i + EXT) as usize] = smulbb(i32::from(out1 as i16), quant_step_size_q16) >> 16;
    }

    let mut ind = [[0i8; MAX_LPC_ORDER]; N];
    let mut prev_out_q10 = [0i16; 2 * N];
    let mut rd_q25 = [0i32; 2 * N];
    let mut rd_min_q25 = [0i32; N];
    let mut rd_max_q25 = [0i32; N];
    let mut ind_sort = [0usize; N];
    let mut n_states = 1;
    for i in (0..order).rev() {
        let rates_q5 = &ec_rates_q5[ec_ix[i] as usize..];
        let in_q10 = i32::from(x_q10[i]);
        for j in 0..n_states {
            let pred_q10 = smulbb(i32::from(pred_coef_q8[i] as i16), prev_out_q10[j].into()) >> 8;
            let res_q10 = i32::from((in_q10 - pred_q10) as i16);
            let ind_tmp = limit_i(smulbb(inv_quant_step_size_q6, res_q10) >> 16, -EXT, EXT - 1);
            ind[j][i] = ind_tmp as i8;
            let out0 = (out0_table[(ind_tmp + EXT) as usize] + pred_q10) as i16;
            let out1 = (out1_table[(ind_tmp + EXT) as usize] + pred_q10) as i16;
            prev_out_q10[j] = out0;
            prev_out_q10[j + n_states] = out1;
            let rate = |k: i32| i32::from(rates_q5[k as usize]);
            let (rate0_q5, rate1_q5) = if ind_tmp + 1 >= NLSF_QUANT_MAX_AMPLITUDE {
                if ind_tmp + 1 == NLSF_QUANT_MAX_AMPLITUDE {
                    (rate(ind_tmp + NLSF_QUANT_MAX_AMPLITUDE), 280)
                } else {
                    let r0 = smlabb(280 - 43 * NLSF_QUANT_MAX_AMPLITUDE, 43, ind_tmp);
                    (r0, r0 + 43)
                }
            } else if ind_tmp <= -NLSF_QUANT_MAX_AMPLITUDE {
                if ind_tmp == -NLSF_QUANT_MAX_AMPLITUDE {
                    (280, rate(ind_tmp + 1 + NLSF_QUANT_MAX_AMPLITUDE))
                } else {
                    let r0 = smlabb(280 - 43 * NLSF_QUANT_MAX_AMPLITUDE, -43, ind_tmp);
                    (r0, r0 - 43)
                }
            } else {
                (
                    rate(ind_tmp + NLSF_QUANT_MAX_AMPLITUDE),
                    rate(ind_tmp + 1 + NLSF_QUANT_MAX_AMPLITUDE),
                )
            };
            let rd_tmp_q25 = rd_q25[j];
            let w = i32::from(w_q5[i]);
            let diff_q10 = i32::from((in_q10 - i32::from(out0)) as i16);
            rd_q25[j] = smlabb(
                rd_tmp_q25.wrapping_add(smulbb(diff_q10, diff_q10).wrapping_mul(w)),
                mu_q20,
                rate0_q5,
            );
            let diff_q10 = i32::from((in_q10 - i32::from(out1)) as i16);
            rd_q25[j + n_states] = smlabb(
                rd_tmp_q25.wrapping_add(smulbb(diff_q10, diff_q10).wrapping_mul(w)),
                mu_q20,
                rate1_q5,
            );
        }
        if n_states <= N / 2 {
            for j in 0..n_states {
                ind[j + n_states][i] = ind[j][i] + 1;
            }
            n_states <<= 1;
            for j in n_states..N {
                ind[j][i] = ind[j - n_states][i];
            }
        } else {
            for j in 0..N {
                if rd_q25[j] > rd_q25[j + N] {
                    rd_max_q25[j] = rd_q25[j];
                    rd_min_q25[j] = rd_q25[j + N];
                    rd_q25[j] = rd_min_q25[j];
                    rd_q25[j + N] = rd_max_q25[j];
                    prev_out_q10.swap(j, j + N);
                    ind_sort[j] = j + N;
                } else {
                    rd_min_q25[j] = rd_q25[j];
                    rd_max_q25[j] = rd_q25[j + N];
                    ind_sort[j] = j;
                }
            }
            loop {
                let mut min_max_q25 = i32::MAX;
                let mut max_min_q25 = 0;
                let mut ind_min_max = 0;
                let mut ind_max_min = 0;
                for j in 0..N {
                    if min_max_q25 > rd_max_q25[j] {
                        min_max_q25 = rd_max_q25[j];
                        ind_min_max = j;
                    }
                    if max_min_q25 < rd_min_q25[j] {
                        max_min_q25 = rd_min_q25[j];
                        ind_max_min = j;
                    }
                }
                if min_max_q25 >= max_min_q25 {
                    break;
                }
                ind_sort[ind_max_min] = ind_sort[ind_min_max] ^ N;
                rd_q25[ind_max_min] = rd_q25[ind_min_max + N];
                prev_out_q10[ind_max_min] = prev_out_q10[ind_min_max + N];
                rd_min_q25[ind_max_min] = 0;
                rd_max_q25[ind_min_max] = i32::MAX;
                ind[ind_max_min] = ind[ind_min_max];
            }
            for j in 0..N {
                ind[j][i] += (ind_sort[j] >> NLSF_QUANT_DEL_DEC_STATES_LOG2) as i8;
            }
        }
    }
    let mut ind_tmp = 0;
    let mut min_q25 = i32::MAX;
    for (j, &rd) in rd_q25.iter().enumerate() {
        if min_q25 > rd {
            min_q25 = rd;
            ind_tmp = j;
        }
    }
    indices[..order].copy_from_slice(&ind[ind_tmp & (N - 1)][..order]);
    indices[0] += (ind_tmp >> NLSF_QUANT_DEL_DEC_STATES_LOG2) as i8;
    min_q25
}

fn limit_i(a: i32, lo: i32, hi: i32) -> i32 {
    a.clamp(lo, hi)
}

/// `silk_NLSF_encode`: quantises `nlsf_q15` in place (stabilised, then the decoded
/// quantisation) and writes its indices; returns the rate-distortion in Q25.
pub(super) fn nlsf_encode(
    nlsf_indices: &mut [i8; MAX_LPC_ORDER + 1],
    nlsf_q15: &mut [i16; MAX_LPC_ORDER],
    cb: &NlsfCodebook,
    w_q2: &[i16],
    nlsf_mu_q20: i32,
    n_survivors: usize,
    signal_type: i32,
) -> i32 {
    let order = cb.order;
    let (inv_step_q6, rates_q5) = enc_fields(cb);
    nlsf_stabilize(&mut nlsf_q15[..order], cb.delta_min_q15);

    let mut err_q24 = [0i32; 32];
    nlsf_vq(&mut err_q24, &nlsf_q15[..order], cb);
    let n_survivors = n_survivors.clamp(1, cb.n_vectors);
    let mut temp_indices1 = [0usize; 32];
    insertion_sort_increasing(
        &mut err_q24[..cb.n_vectors],
        &mut temp_indices1[..n_survivors],
    );

    let mut rd_q25 = [0i32; 32];
    let mut temp_indices2 = [[0i8; MAX_LPC_ORDER]; 32];
    for s in 0..n_survivors {
        let ind1 = temp_indices1[s];
        let cb_el = &cb.cb1_nlsf_q8[ind1 * order..][..order];
        let cb_w = &cb.cb1_wght_q9[ind1 * order..][..order];
        let mut res_q10 = [0i16; MAX_LPC_ORDER];
        let mut w_adj_q5 = [0i16; MAX_LPC_ORDER];
        for i in 0..order {
            let tmp_q15 = i32::from((u16::from(cb_el[i]) << 7) as i16);
            let w_tmp_q9 = i32::from(cb_w[i]);
            res_q10[i] = (smulbb(i32::from(nlsf_q15[i]) - tmp_q15, w_tmp_q9) >> 14) as i16;
            w_adj_q5[i] = div32_varq(i32::from(w_q2[i]), smulbb(w_tmp_q9, w_tmp_q9), 21) as i16;
        }
        let mut ec_ix = [0i16; MAX_LPC_ORDER];
        let mut pred_q8 = [0u8; MAX_LPC_ORDER];
        nlsf_unpack(&mut ec_ix, &mut pred_q8, cb, ind1);
        rd_q25[s] = nlsf_del_dec_quant(
            &mut temp_indices2[s],
            &res_q10,
            &w_adj_q5,
            &pred_q8,
            &ec_ix,
            rates_q5,
            cb.quant_step_size_q16,
            inv_step_q6,
            nlsf_mu_q20,
            order,
        );
        let icdf = &cb.cb1_icdf[(signal_type >> 1) as usize * cb.n_vectors..];
        let prob_q8 = if ind1 == 0 {
            256 - i32::from(icdf[0])
        } else {
            i32::from(icdf[ind1 - 1]) - i32::from(icdf[ind1])
        };
        let bits_q7 = (8 << 7) - lin2log(prob_q8);
        rd_q25[s] = smlabb(rd_q25[s], bits_q7, nlsf_mu_q20 >> 2);
    }
    let mut best = [0usize; 1];
    insertion_sort_increasing(&mut rd_q25[..n_survivors], &mut best);
    nlsf_indices[0] = temp_indices1[best[0]] as i8;
    nlsf_indices[1..=order].copy_from_slice(&temp_indices2[best[0]][..order]);
    nlsf_decode(nlsf_q15, nlsf_indices, cb);
    rd_q25[0]
}

#[cfg(test)]
#[path = "nlsf_enc_tests.rs"]
mod tests;
