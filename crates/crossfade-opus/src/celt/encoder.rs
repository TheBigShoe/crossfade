//! `celt_encode_with_ec` (`celt/celt_encoder.c`, float build): the frame's analysis (transients,
//! the time-frequency resolution, the dynamic allocation, spreading, the stereo decisions) and
//! its symbols in the order the decoder reads them.
//!
//! The encoder owns the choices the format leaves free; none of them needs to match the
//! reference's bit for bit, and every one is checked by decoding: [`CeltDecoder`] must end
//! each frame on this encoder's final range.
//!
//! [`CeltDecoder`]: super::CeltDecoder
#![allow(clippy::needless_range_loop)] // index loops as the reference writes them

use super::CeltFrame;
use super::bands::{AllBands, haar1, quant_all_bands};
use super::ec::Enc;
use super::lpc::{pitch_downsample, pitch_search, remove_doubling};
use super::maths::exp2;
use super::mdct;
use super::quant_bands::{
    CoarseEnergy, amp2_log2, quant_coarse_energy, quant_energy_finalise, quant_fine_energy,
};
use super::rate::{AllocHint, BITRES, compute_allocation, init_caps};
use super::tables::{
    COMB_GAINS, E_MEANS, EBANDS, LOG_N, NB_EBANDS, OVERLAP, PREEMPH, SHORT_MDCT_SIZE, SPREAD_ICDF,
    TAPSET_ICDF, TF_SELECT_TABLE, TRIM_ICDF, WINDOW,
};
use crate::error::{Error, Result};
use crate::range_encoder::RangeEncoder;

/// The pitch filter's periods (`COMBFILTER_MINPERIOD`, `COMBFILTER_MAXPERIOD`).
const MIN_PERIOD: usize = 15;
const MAX_PERIOD: usize = 1024;
const SPREAD_NONE: i32 = 0;
const SPREAD_LIGHT: i32 = 1;
const SPREAD_NORMAL: i32 = 2;
const SPREAD_AGGRESSIVE: i32 = 3;
/// The largest frame CELT writes.
const MAX_FRAME_BYTES: usize = 1275;
/// The longest frame, in samples per channel.
const MAX_N: usize = 960;
const EPSILON: f32 = 1e-15;

fn eb(i: usize) -> usize {
    EBANDS[i] as usize
}

/// The CELT encoder's state for one stream (libopus `CELTEncoder`).
#[derive(Debug, Clone)]
pub(crate) struct CeltEncoder {
    /// Input channels, 1 or 2 (`CC`).
    channels: usize,
    /// 0 to 10: which analyses run (`OPUS_SET_COMPLEXITY`).
    complexity: i32,
    /// Bits of the input that carry signal: where silence and the noise floor are.
    lsb_depth: i32,
    disable_inv: bool,
    /// The target in bits per second when the bitrate is variable.
    vbr: Option<i32>,
    /// A low-frequency effects channel: everything of interest is in the first band.
    lfe: bool,
    rng: u32,
    spread_decision: i32,
    delayed_intra: f32,
    tonal_average: i32,
    last_coded_bands: i32,
    hf_average: i32,
    tapset_decision: i32,
    consec_transient: i32,
    preemph_mem: [f32; 2],
    overlap_max: f32,
    stereo_saving: f32,
    intensity: usize,
    spec_avg: f32,
    /// The last `OVERLAP` pre-filtered samples of each channel (`in_mem`).
    in_mem: [[f32; OVERLAP]; 2],
    /// The last `MAX_PERIOD` pre-emphasised samples of each channel, before the pitch filter.
    prefilter_mem: [Vec<f32>; 2],
    prefilter_period: usize,
    prefilter_gain: f32,
    prefilter_tapset: usize,
    old_band_e: [f32; 2 * NB_EBANDS],
    old_log_e: [f32; 2 * NB_EBANDS],
    old_log_e2: [f32; 2 * NB_EBANDS],
    energy_error: [f32; 2 * NB_EBANDS],
}

/// `hysteresis_decision`: the index of the first threshold above `val`, held at `prev` while
/// `val` is within that step's hysteresis.
fn hysteresis_decision(val: f32, thresholds: &[f32], hysteresis: &[f32], prev: usize) -> usize {
    let n = thresholds.len();
    let mut i = thresholds.iter().position(|&t| val < t).unwrap_or(n);
    let prev = prev.min(n);
    if i > prev && prev < n && val < thresholds[prev] + hysteresis[prev] {
        i = prev;
    }
    if i < prev && val > thresholds[prev - 1] - hysteresis[prev - 1] {
        i = prev;
    }
    i
}

/// What `transient_analysis` finds in a frame.
struct Transient {
    is_transient: bool,
    /// 0 for a stationary frame to about 1 for a strong attack.
    tf_estimate: f32,
    /// The channel that showed it most.
    tf_chan: usize,
}

/// `transient_analysis`: a high-passed energy envelope with forward and backward masking;
/// a frame whose energy stands far above what masks it needs short blocks.
fn transient_analysis(input: &[[f32; MAX_N + OVERLAP]; 2], len: usize, c: usize) -> Transient {
    const INV_TABLE: [u8; 128] = [
        255, 255, 156, 110, 86, 70, 59, 51, 45, 40, 37, 33, 31, 28, 26, 25, 23, 22, 21, 20, 19, 18,
        17, 16, 16, 15, 15, 14, 13, 13, 12, 12, 12, 12, 11, 11, 11, 10, 10, 10, 9, 9, 9, 9, 9, 9,
        8, 8, 8, 8, 8, 7, 7, 7, 7, 7, 7, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 5, 5, 5,
        5, 5, 5, 5, 5, 5, 5, 5, 5, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
        4, 4, 4, 4, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 2,
    ];
    let forward_decay = 0.0625f32;
    let len2 = len / 2;
    let mut tmp = [0.0f32; MAX_N + OVERLAP];
    let mut mask_metric = 0i32;
    let mut tf_chan = 0;
    for ch in 0..c {
        let (mut mem0, mut mem1) = (0.0f32, 0.0f32);
        for i in 0..len {
            let x = input[ch][i];
            let y = mem0 + x;
            mem0 = mem1 + y - 2.0 * x;
            mem1 = x - 0.5 * y;
            tmp[i] = y;
        }
        // The filter's start-up is not a transient.
        tmp[..12].fill(0.0);
        let mut mean = 0.0f32;
        mem0 = 0.0;
        // Forward masking: 6.7 dB/ms.
        for i in 0..len2 {
            let x2 = tmp[2 * i] * tmp[2 * i] + tmp[2 * i + 1] * tmp[2 * i + 1];
            mean += x2;
            tmp[i] = mem0 + forward_decay * (x2 - mem0);
            mem0 = tmp[i];
        }
        mem0 = 0.0;
        let mut max_e = 0.0f32;
        // Backward masking: 13.9 dB/ms.
        for i in (0..len2).rev() {
            tmp[i] = mem0 + 0.125 * (tmp[i] - mem0);
            mem0 = tmp[i];
            max_e = max_e.max(mem0);
        }
        // The frame's energy against the harmonic mean of the envelope: the geometric mean
        // of the energy and half the peak stands for the former.
        let mean = (f64::from(mean) * f64::from(max_e) * 0.5 * len2 as f64).sqrt() as f32;
        let norm = len2 as f32 / (EPSILON + mean);
        let mut unmask = 0i32;
        let mut i = 12;
        while i < len2 - 5 {
            let id = (64.0 * norm * (tmp[i] + EPSILON)).floor().clamp(0.0, 127.0) as usize;
            unmask += i32::from(INV_TABLE[id.min(127)]);
            i += 4;
        }
        let unmask = 64 * unmask * 4 / (6 * (len2 as i32 - 17));
        if unmask > mask_metric {
            tf_chan = ch;
            mask_metric = unmask;
        }
    }
    let tf_max = (((27 * mask_metric) as f32).sqrt() - 42.0).max(0.0);
    let tf_estimate = (0.0069 * tf_max.min(163.0) - 0.139).max(0.0).sqrt();
    Transient {
        is_transient: mask_metric > 200,
        tf_estimate,
        tf_chan,
    }
}

/// `patch_transient_decision`: a sudden rise of the band energies over the last frame's
/// (spread by 6 dB a band) that the time-domain analysis missed.
fn patch_transient_decision(
    new_e: &[f32; 2 * NB_EBANDS],
    old_e: &[f32; 2 * NB_EBANDS],
    start: usize,
    end: usize,
    c: usize,
) -> bool {
    let mut spread_old = [0.0f32; NB_EBANDS];
    if c == 1 {
        spread_old[start] = old_e[start];
        for i in start + 1..end {
            spread_old[i] = (spread_old[i - 1] - 1.0).max(old_e[i]);
        }
    } else {
        spread_old[start] = old_e[start].max(old_e[start + NB_EBANDS]);
        for i in start + 1..end {
            spread_old[i] = (spread_old[i - 1] - 1.0).max(old_e[i].max(old_e[i + NB_EBANDS]));
        }
    }
    for i in (start..end.saturating_sub(1)).rev() {
        spread_old[i] = spread_old[i].max(spread_old[i + 1] - 1.0);
    }
    let from = start.max(2);
    if end < from + 2 {
        return false;
    }
    let mut mean_diff = 0.0f32;
    for ch in 0..c {
        for i in from..end - 1 {
            let x1 = new_e[i + ch * NB_EBANDS].max(0.0);
            let x2 = spread_old[i].max(0.0);
            mean_diff += (x1 - x2).max(0.0);
        }
    }
    mean_diff /= (c * (end - 1 - from)) as f32;
    mean_diff > 1.0
}

/// `compute_mdcts`: the MDCT of each channel's `n + OVERLAP` samples, one long block or
/// `1 << lm` short ones interleaved; two channels coded as one are mixed.
fn compute_mdcts(
    short_blocks: bool,
    input: &[[f32; MAX_N + OVERLAP]; 2],
    freq: &mut [f32; 2 * MAX_N],
    c: usize,
    cc: usize,
    lm: usize,
) {
    let (b, n, shift) = if short_blocks {
        (1usize << lm, SHORT_MDCT_SIZE, 3)
    } else {
        (1, SHORT_MDCT_SIZE << lm, 3 - lm)
    };
    let total = b * n;
    for ch in 0..cc {
        for blk in 0..b {
            mdct::forward(
                &input[ch][blk * n..blk * n + n + OVERLAP],
                &mut freq[ch * total + blk..(ch + 1) * total],
                b,
                shift,
            );
        }
    }
    if cc == 2 && c == 1 {
        for i in 0..total {
            freq[i] = 0.5 * freq[i] + 0.5 * freq[total + i];
        }
    }
}

/// `compute_band_energies`: each band's amplitude.
fn compute_band_energies(
    freq: &[f32; 2 * MAX_N],
    band_e: &mut [f32; 2 * NB_EBANDS],
    end: usize,
    c: usize,
    lm: usize,
) {
    let n = SHORT_MDCT_SIZE << lm;
    for ch in 0..c {
        for i in 0..end {
            let band = &freq[ch * n + (eb(i) << lm)..ch * n + (eb(i + 1) << lm)];
            let mut sum = 1e-27f32;
            for &v in band {
                sum += v * v;
            }
            band_e[i + ch * NB_EBANDS] = sum.sqrt();
        }
    }
}

/// `normalise_bands`: each band scaled to unit norm.
fn normalise_bands(
    freq: &[f32; 2 * MAX_N],
    x: &mut [f32; 2 * MAX_N],
    band_e: &[f32; 2 * NB_EBANDS],
    end: usize,
    c: usize,
    lm: usize,
) {
    let n = SHORT_MDCT_SIZE << lm;
    for ch in 0..c {
        for i in 0..end {
            let g = 1.0 / (1e-27 + band_e[i + ch * NB_EBANDS]);
            for j in ch * n + (eb(i) << lm)..ch * n + (eb(i + 1) << lm) {
                x[j] = freq[j] * g;
            }
        }
        x[ch * n + (eb(end) << lm)..(ch + 1) * n].fill(0.0);
    }
}

/// `l1_metric`: the L1 norm, biased against time resolution it has to pay for.
fn l1_metric(tmp: &[f32], lm: i32, bias: f32) -> f32 {
    let mut l1 = 0.0f32;
    for &v in tmp {
        l1 += v.abs();
    }
    l1 + lm as f32 * bias * l1
}

/// `tf_analysis`: per band, whether to trade frequency for time resolution (or back), by a
/// Viterbi search over the L1 norms of the band at each resolution. Returns `tf_select`.
#[allow(clippy::too_many_arguments)]
fn tf_analysis(
    len: usize,
    transient: bool,
    tf_res: &mut [i32; NB_EBANDS],
    lambda: i32,
    x: &[f32; 2 * MAX_N],
    n0: usize,
    lm: usize,
    tf_estimate: f32,
    tf_chan: usize,
    importance: &[i32; NB_EBANDS],
) -> usize {
    let lmi = lm as i32;
    let bias = 0.04 * (0.5 - tf_estimate).max(-0.25);
    let mut metric = [0i32; NB_EBANDS];
    let mut tmp = [0.0f32; 176];
    let mut tmp_1 = [0.0f32; 176];
    for i in 0..len {
        let n = (eb(i + 1) - eb(i)) << lm;
        let narrow = eb(i + 1) - eb(i) == 1;
        let at = tf_chan * n0 + (eb(i) << lm);
        tmp[..n].copy_from_slice(&x[at..at + n]);
        let mut best_l1 = l1_metric(&tmp[..n], if transient { lmi } else { 0 }, bias);
        let mut best_level = 0i32;
        // One more step of time resolution than the short blocks have.
        if transient && !narrow {
            tmp_1[..n].copy_from_slice(&tmp[..n]);
            haar1(&mut tmp_1[..n], n >> lm, 1 << lm);
            let l1 = l1_metric(&tmp_1[..n], lmi + 1, bias);
            if l1 < best_l1 {
                best_l1 = l1;
                best_level = -1;
            }
        }
        let levels = lm + usize::from(!(transient || narrow));
        for k in 0..levels {
            let b = if transient {
                lmi - k as i32 - 1
            } else {
                k as i32 + 1
            };
            haar1(&mut tmp[..n], n >> k, 1 << k);
            let l1 = l1_metric(&tmp[..n], b, bias);
            if l1 < best_l1 {
                best_l1 = l1;
                best_level = k as i32 + 1;
            }
        }
        metric[i] = if transient {
            2 * best_level
        } else {
            -2 * best_level
        };
        // A band that cannot be split is put half-way, so that it does not bias the choice.
        if narrow && (metric[i] == 0 || metric[i] == -2 * lmi) {
            metric[i] -= 1;
        }
    }
    let t = 4 * usize::from(transient);
    let table = |sel: usize, r: usize| i32::from(TF_SELECT_TABLE[lm][t + 2 * sel + r]);
    let entry = if transient { 0 } else { lambda };
    let mut selcost = [0i32; 2];
    for sel in 0..2 {
        let mut cost0 = importance[0] * (metric[0] - 2 * table(sel, 0)).abs();
        let mut cost1 = importance[0] * (metric[0] - 2 * table(sel, 1)).abs() + entry;
        for i in 1..len {
            let curr0 = cost0.min(cost1 + lambda);
            let curr1 = (cost0 + lambda).min(cost1);
            cost0 = curr0 + importance[i] * (metric[i] - 2 * table(sel, 0)).abs();
            cost1 = curr1 + importance[i] * (metric[i] - 2 * table(sel, 1)).abs();
        }
        selcost[sel] = cost0.min(cost1);
    }
    // Only transients may use the second table, for now.
    let tf_select = usize::from(selcost[1] < selcost[0] && transient);
    let mut path0 = [0i32; NB_EBANDS];
    let mut path1 = [0i32; NB_EBANDS];
    let mut cost0 = importance[0] * (metric[0] - 2 * table(tf_select, 0)).abs();
    let mut cost1 = importance[0] * (metric[0] - 2 * table(tf_select, 1)).abs() + entry;
    for i in 1..len {
        let (from0, from1) = (cost0, cost1 + lambda);
        let curr0 = if from0 < from1 {
            path0[i] = 0;
            from0
        } else {
            path0[i] = 1;
            from1
        };
        let (from0, from1) = (cost0 + lambda, cost1);
        let curr1 = if from0 < from1 {
            path1[i] = 0;
            from0
        } else {
            path1[i] = 1;
            from1
        };
        cost0 = curr0 + importance[i] * (metric[i] - 2 * table(tf_select, 0)).abs();
        cost1 = curr1 + importance[i] * (metric[i] - 2 * table(tf_select, 1)).abs();
    }
    tf_res[len - 1] = i32::from(cost0 >= cost1);
    for i in (0..len - 1).rev() {
        tf_res[i] = if tf_res[i + 1] == 1 {
            path1[i + 1]
        } else {
            path0[i + 1]
        };
    }
    tf_select
}

/// `tf_encode`: the per-band flags and the table choice, as far as the budget goes; `tf_res`
/// becomes the change of resolution per band that the decoder will apply.
fn tf_encode(
    start: usize,
    end: usize,
    transient: bool,
    tf_res: &mut [i32; NB_EBANDS],
    lm: usize,
    mut tf_select: usize,
    enc: &mut Enc<'_>,
) {
    let mut budget = (enc.storage() * 8) as u32;
    let mut tell = enc.tell() as u32;
    let mut logp: u32 = if transient { 2 } else { 4 };
    let tf_select_rsv = lm > 0 && tell + logp < budget;
    budget -= u32::from(tf_select_rsv);
    let mut curr = 0;
    let mut tf_changed = 0;
    for r in tf_res.iter_mut().take(end).skip(start) {
        if tell + logp <= budget {
            enc.bit_logp((*r ^ curr) != 0, logp);
            tell = enc.tell() as u32;
            curr = *r;
            tf_changed |= curr;
        } else {
            *r = curr;
        }
        logp = if transient { 4 } else { 5 };
    }
    let t = 4 * usize::from(transient);
    if tf_select_rsv
        && TF_SELECT_TABLE[lm][t + tf_changed as usize]
            != TF_SELECT_TABLE[lm][t + 2 + tf_changed as usize]
    {
        enc.bit_logp(tf_select != 0, 1);
    } else {
        tf_select = 0;
    }
    for r in tf_res.iter_mut().take(end).skip(start) {
        *r = i32::from(TF_SELECT_TABLE[lm][t + 2 * tf_select + *r as usize]);
    }
}

/// `spreading_decision`: how much to rotate the pulses, from how peaky the bands are (tonal
/// frames want none), with the recursive average and hysteresis the reference keeps; and, when
/// the pitch filter is on, its tapset from the top bands.
#[allow(clippy::too_many_arguments)]
fn spreading_decision(
    x: &[f32; 2 * MAX_N],
    average: &mut i32,
    last_decision: i32,
    hf_average: &mut i32,
    tapset_decision: &mut i32,
    update_hf: bool,
    end: usize,
    c: usize,
    lm: usize,
    spread_weight: &[i32; NB_EBANDS],
) -> i32 {
    let m = 1usize << lm;
    let n0 = m * SHORT_MDCT_SIZE;
    if m * (eb(end) - eb(end - 1)) <= 8 {
        return SPREAD_NONE;
    }
    let mut sum = 0i32;
    let mut nb_bands = 0i32;
    let mut hf_sum = 0i32;
    for ch in 0..c {
        for i in 0..end {
            let n = m * (eb(i + 1) - eb(i));
            if n <= 8 {
                continue;
            }
            let band = &x[ch * n0 + m * eb(i)..ch * n0 + m * eb(i + 1)];
            // A rough distribution of |x|.
            let mut tcount = [0i32; 3];
            for &v in band {
                let x2n = v * v * n as f32;
                tcount[0] += i32::from(x2n < 0.25);
                tcount[1] += i32::from(x2n < 0.0625);
                tcount[2] += i32::from(x2n < 0.015625);
            }
            let n = n as i32;
            // The four last bands only (8 kHz and up).
            if i > NB_EBANDS - 4 {
                hf_sum += 32 * (tcount[1] + tcount[0]) / n;
            }
            let tmp = i32::from(2 * tcount[2] >= n)
                + i32::from(2 * tcount[1] >= n)
                + i32::from(2 * tcount[0] >= n);
            sum += tmp * spread_weight[i];
            nb_bands += spread_weight[i];
        }
    }
    if update_hf {
        if hf_sum != 0 {
            hf_sum /= (c * (4 + end).saturating_sub(NB_EBANDS)).max(1) as i32;
        }
        *hf_average = (*hf_average + hf_sum) >> 1;
        hf_sum = *hf_average;
        if *tapset_decision == 2 {
            hf_sum += 4;
        } else if *tapset_decision == 0 {
            hf_sum -= 4;
        }
        *tapset_decision = if hf_sum > 22 {
            2
        } else if hf_sum > 18 {
            1
        } else {
            0
        };
    }
    if nb_bands == 0 {
        return last_decision;
    }
    let sum = (sum << 8) / nb_bands;
    let sum = (sum + *average) >> 1;
    *average = sum;
    let sum = (3 * sum + (((3 - last_decision) << 7) + 64) + 2) >> 2;
    if sum < 80 {
        SPREAD_AGGRESSIVE
    } else if sum < 256 {
        SPREAD_NORMAL
    } else if sum < 384 {
        SPREAD_LIGHT
    } else {
        SPREAD_NONE
    }
}

/// `celt_log2`.
fn log2(x: f32) -> f32 {
    (std::f64::consts::LOG2_E * libm::log(f64::from(x))) as f32
}

/// `alloc_trim_analysis`: tilts the allocation towards the low bands (a high trim) or the
/// high ones, from the spectrum's slope, the channels' correlation and the transient estimate.
#[allow(clippy::too_many_arguments)]
fn alloc_trim_analysis(
    x: &[f32; 2 * MAX_N],
    band_log_e: &[f32; 2 * NB_EBANDS],
    end: usize,
    lm: usize,
    c: usize,
    n0: usize,
    stereo_saving: &mut f32,
    tf_estimate: f32,
    intensity: usize,
    equiv_rate: i32,
) -> i32 {
    // A lower trim helps at low rates.
    let mut trim = 5.0f32;
    if equiv_rate < 64000 {
        trim = 4.0;
    } else if equiv_rate < 80000 {
        let frac = (equiv_rate - 64000) >> 10;
        trim = 4.0 + (1.0 / 16.0) * frac as f32;
    }
    if c == 2 {
        let partial = |i: usize| -> f32 {
            let mut p = 0.0f32;
            for j in eb(i) << lm..eb(i + 1) << lm {
                p += x[j] * x[n0 + j];
            }
            p
        };
        // The inter-channel correlation of the low bands.
        let mut sum = 0.0f32;
        for i in 0..8 {
            sum += partial(i);
        }
        let sum = ((1.0 / 8.0) * sum).abs().min(1.0);
        let mut min_xc = sum;
        for i in 8..intensity.min(NB_EBANDS) {
            min_xc = min_xc.min(partial(i).abs());
        }
        let min_xc = min_xc.abs().min(1.0);
        // Mid-side savings from the LF correlation, in bits per sample.
        let log_xc = log2(1.001 - sum * sum);
        let log_xc2 = (0.5 * log_xc).max(log2(1.001 - min_xc * min_xc));
        trim += (0.75 * log_xc).max(-4.0);
        *stereo_saving = (*stereo_saving + 0.25).min(-(0.5 * log_xc2));
    }
    // The spectral tilt.
    let mut diff = 0.0f32;
    for ch in 0..c {
        for i in 0..end - 1 {
            diff += band_log_e[i + ch * NB_EBANDS] * (2 + 2 * i as i32 - end as i32) as f32;
        }
    }
    diff /= (c * (end - 1)) as f32;
    trim -= ((diff + 1.0) / 6.0).clamp(-2.0, 2.0);
    trim -= 2.0 * tf_estimate;
    ((0.5 + trim).floor() as i32).clamp(0, 10)
}

/// `stereo_analysis`: whether left/right (dual stereo) would cost less than mid/side, from
/// the L1 norms of both over the low bands.
fn stereo_analysis(x: &[f32; 2 * MAX_N], lm: usize, n0: usize) -> bool {
    let mut sum_lr = EPSILON;
    let mut sum_ms = EPSILON;
    for i in 0..13 {
        for j in eb(i) << lm..eb(i + 1) << lm {
            let l = x[j];
            let r = x[n0 + j];
            sum_lr += l.abs() + r.abs();
            sum_ms += (l + r).abs() + (l - r).abs();
        }
    }
    let sum_ms = std::f32::consts::FRAC_1_SQRT_2 * sum_ms;
    // No bits to code the angles with at short frames: mid/side must win by more.
    let thetas = if lm <= 1 { 5.0 } else { 13.0 };
    let base = (eb(13) << (lm + 1)) as f32;
    (base + thetas) * sum_ms > base * sum_lr
}

fn median_of_5(x: &[f32]) -> f32 {
    let mut v = [x[0], x[1], x[2], x[3], x[4]];
    v.sort_by(f32::total_cmp);
    v[2]
}

fn median_of_3(x: &[f32]) -> f32 {
    let mut v = [x[0], x[1], x[2]];
    v.sort_by(f32::total_cmp);
    v[1]
}

/// What `dynalloc_analysis` decides beyond the boosts it leaves in `offsets`.
struct Dynalloc {
    /// The most any band stands above the noise floor.
    max_depth: f32,
    /// Bits the boosts take, in 1/8 bit.
    tot_boost: i32,
    /// Per band, how much the time-frequency choice matters there.
    importance: [i32; NB_EBANDS],
    /// Per band, its weight in the spreading decision (masked bands count less).
    spread_weight: [i32; NB_EBANDS],
}

/// `dynalloc_analysis`: bands that stand out above a smoothed spectral
/// envelope get extra bits (`offsets`, in steps the bitstream's boost flags express).
#[allow(clippy::too_many_arguments)]
fn dynalloc_analysis(
    band_log_e: &[f32; 2 * NB_EBANDS],
    band_log_e2: &[f32; 2 * NB_EBANDS],
    start: usize,
    end: usize,
    c: usize,
    offsets: &mut [i32; NB_EBANDS],
    lsb_depth: i32,
    transient: bool,
    vbr: bool,
    lm: usize,
    effective_bytes: i32,
) -> Dynalloc {
    let mut out = Dynalloc {
        max_depth: -31.9,
        tot_boost: 0,
        importance: [13; NB_EBANDS],
        spread_weight: [32; NB_EBANDS],
    };
    *offsets = [0; NB_EBANDS];
    let mut noise_floor = [0.0f32; NB_EBANDS];
    for i in 0..end {
        // The mean, the depth, the band's width and the pre-emphasis (about the square of
        // the band's Bark index) all move the floor.
        noise_floor[i] = 0.0625 * f32::from(LOG_N[i]) + 0.5 + (9 - lsb_depth) as f32 - E_MEANS[i]
            + 0.0062 * ((i + 5) * (i + 5)) as f32;
    }
    let mut max_depth = -31.9f32;
    for ch in 0..c {
        for i in 0..end {
            max_depth = max_depth.max(band_log_e[ch * NB_EBANDS + i] - noise_floor[i]);
        }
    }
    out.max_depth = max_depth;
    {
        // A simple masking model, so that masked bands weigh less in the spreading decision.
        let mut mask = [0.0f32; NB_EBANDS];
        for i in 0..end {
            mask[i] = band_log_e[i] - noise_floor[i];
            if c == 2 {
                mask[i] = mask[i].max(band_log_e[NB_EBANDS + i] - noise_floor[i]);
            }
        }
        let sig = mask;
        for i in 1..end {
            mask[i] = mask[i].max(mask[i - 1] - 2.0);
        }
        for i in (0..end - 1).rev() {
            mask[i] = mask[i].max(mask[i + 1] - 3.0);
        }
        for i in 0..end {
            let smr = sig[i] - (max_depth - 12.0).max(0.0).max(mask[i]);
            let shift = (-((0.5 + smr).floor() as i32)).clamp(0, 5);
            out.spread_weight[i] = 32 >> shift;
        }
    }
    if effective_bytes <= 50 || lm < 1 || end < 5 {
        return out;
    }
    let mut follower = [0.0f32; 2 * NB_EBANDS];
    let mut last = 0;
    for ch in 0..c {
        let e2 = &band_log_e2[ch * NB_EBANDS..(ch + 1) * NB_EBANDS];
        let f = &mut follower[ch * NB_EBANDS..(ch + 1) * NB_EBANDS];
        f[0] = e2[0];
        for i in 1..end {
            // The last band at least 3 dB above the one before is the last considered, for
            // band-limited signals.
            if e2[i] > e2[i - 1] + 0.5 {
                last = i;
            }
            f[i] = (f[i - 1] + 1.5).min(e2[i]);
        }
        for i in (0..last).rev() {
            f[i] = f[i].min((f[i + 1] + 2.0).min(e2[i]));
        }
        // A median filter keeps dynalloc from triggering needlessly.
        let offset = 1.0f32;
        for i in 2..end - 2 {
            f[i] = f[i].max(median_of_5(&e2[i - 2..]) - offset);
        }
        let tmp = median_of_3(&e2[..3]) - offset;
        f[0] = f[0].max(tmp);
        f[1] = f[1].max(tmp);
        let tmp = median_of_3(&e2[end - 3..]) - offset;
        f[end - 2] = f[end - 2].max(tmp);
        f[end - 1] = f[end - 1].max(tmp);
        for i in 0..end {
            f[i] = f[i].max(noise_floor[i]);
        }
    }
    if c == 2 {
        for i in start..end {
            // Cross-talk between the channels: 24 dB.
            follower[NB_EBANDS + i] = follower[NB_EBANDS + i].max(follower[i] - 4.0);
            follower[i] = follower[i].max(follower[NB_EBANDS + i] - 4.0);
            follower[i] = 0.5
                * ((band_log_e[i] - follower[i]).max(0.0)
                    + (band_log_e[NB_EBANDS + i] - follower[NB_EBANDS + i]).max(0.0));
        }
    } else {
        for i in start..end {
            follower[i] = (band_log_e[i] - follower[i]).max(0.0);
        }
    }
    for i in start..end {
        out.importance[i] = (0.5 + 13.0 * exp2(follower[i].min(4.0))).floor() as i32;
    }
    // Constant bitrate cannot afford as much, except on transients.
    if !vbr && !transient {
        for i in start..end {
            follower[i] *= 0.5;
        }
    }
    for i in start..end {
        if i < 8 {
            follower[i] *= 2.0;
        }
        if i >= 12 {
            follower[i] *= 0.5;
        }
    }
    let mut tot_boost = 0i32;
    for i in start..end {
        follower[i] = follower[i].min(4.0);
        let width = ((c * (eb(i + 1) - eb(i))) << lm) as i32;
        let (boost, boost_bits);
        if width < 6 {
            boost = follower[i] as i32;
            boost_bits = (boost * width) << BITRES;
        } else if width > 48 {
            boost = (follower[i] * 8.0) as i32;
            boost_bits = ((boost * width) << BITRES) / 8;
        } else {
            boost = (follower[i] * width as f32 / 6.0) as i32;
            boost_bits = (boost * 6) << BITRES;
        }
        // At constant bitrate dynalloc takes at most two thirds of the frame.
        if !vbr && (tot_boost + boost_bits) >> BITRES >> 3 > 2 * effective_bytes / 3 {
            let cap = (2 * effective_bytes / 3) << BITRES << 3;
            offsets[i] = cap - tot_boost;
            tot_boost = cap;
            break;
        }
        offsets[i] = boost;
        tot_boost += boost_bits;
    }
    out.tot_boost = tot_boost;
    out
}

/// `comb_filter` out of place: `dst[i] = src[at + i]` plus the pitch filter's taps on `src`,
/// cross-faded from (`t0`, `g0`, `tapset0`) to (`t1`, `g1`, `tapset1`) over the overlap with
/// the squared window. `src` holds `t + 2` samples before `at`.
#[allow(clippy::too_many_arguments)]
fn comb_filter(
    dst: &mut [f32],
    src: &[f32],
    at: usize,
    t0: usize,
    t1: usize,
    g0: f32,
    g1: f32,
    tapset0: usize,
    tapset1: usize,
) {
    let n = dst.len();
    if g0 == 0.0 && g1 == 0.0 {
        dst.copy_from_slice(&src[at..at + n]);
        return;
    }
    let [g00, g01, g02] = COMB_GAINS[tapset0].map(|g| g0 * g);
    let [g10, g11, g12] = COMB_GAINS[tapset1].map(|g| g1 * g);
    let overlap = OVERLAP.min(n);
    for i in 0..n {
        let x = at + i;
        let new = g10 * src[x - t1]
            + g11 * (src[x - t1 - 1] + src[x - t1 + 1])
            + g12 * (src[x - t1 - 2] + src[x - t1 + 2]);
        dst[i] = if i < overlap {
            let f = WINDOW[i] * WINDOW[i];
            let old = g00 * src[x - t0]
                + g01 * (src[x - t0 - 1] + src[x - t0 + 1])
                + g02 * (src[x - t0 - 2] + src[x - t0 + 2]);
            src[x] + (1.0 - f) * old + f * new
        } else {
            src[x] + new
        };
    }
}

/// What `compute_vbr` weighs beyond the base target.
struct VbrInputs {
    lm: usize,
    equiv_rate: i32,
    last_coded_bands: i32,
    c: usize,
    intensity: usize,
    stereo_saving: f32,
    tot_boost: i32,
    tf_estimate: f32,
    max_depth: f32,
    temporal_vbr: f32,
}

/// `compute_vbr`: the frame's target in 1/8 bit, from `base_target`: less for correlated
/// stereo, more for dynalloc's boosts and for transients, no more than the signal's depth
/// above the noise floor can use, and following the spectrum's level over time.
fn compute_vbr(base_target: i32, v: &VbrInputs) -> i32 {
    let lm = v.lm;
    let coded_bands = if v.last_coded_bands != 0 {
        v.last_coded_bands as usize
    } else {
        NB_EBANDS
    };
    let mut coded_bins = (eb(coded_bands) << lm) as i32;
    if v.c == 2 {
        coded_bins += (eb(v.intensity.min(coded_bands)) << lm) as i32;
    }
    let mut target = base_target;
    if v.c == 2 {
        let coded_stereo_bands = v.intensity.min(coded_bands);
        let coded_stereo_dof = (eb(coded_stereo_bands) << lm) as i32 - coded_stereo_bands as i32;
        // Leave at least 20% of the bits to the mid.
        let max_frac = 0.8 * coded_stereo_dof as f32 / coded_bins as f32;
        let stereo_saving = v.stereo_saving.min(1.0);
        let saved = (max_frac * target as f32)
            .min((stereo_saving - 0.1) * (coded_stereo_dof << BITRES) as f32);
        target -= saved as i32;
    }
    // Dynalloc's boosts, less their average over a calibration set.
    target += v.tot_boost - (19 << lm);
    // Transients, less their average likewise.
    target += ((v.tf_estimate - 0.044) * target as f32) as i32;
    {
        // No more bits than the signal's depth above the floor can use.
        let bins = (eb(NB_EBANDS - 2) << lm) as i32;
        let floor_depth = (((v.c as i32 * bins) << BITRES) as f32 * v.max_depth) as i32;
        let floor_depth = floor_depth.max(target >> 2);
        target = target.min(floor_depth);
    }
    if v.tf_estimate < 0.2 {
        let amount = 0.000_003_1 * (96_000 - v.equiv_rate).clamp(0, 32_000) as f32;
        let tvbr_factor = v.temporal_vbr * amount;
        target += (tvbr_factor * target as f32) as i32;
    }
    // Never more than twice the base rate.
    target.min(2 * base_target)
}

/// What `run_prefilter` chose for the frame.
struct Prefilter {
    on: bool,
    pitch: usize,
    gain: f32,
    /// The gain's 3-bit code.
    qg: u32,
}

impl CeltEncoder {
    /// `run_prefilter`: finds the frame's pitch and, when it is strong enough, removes that
    /// much of it (the decoder's post-filter puts it back, shaping the coding noise around
    /// the harmonics). `input` holds each channel's pre-emphasised frame after `OVERLAP`
    /// samples that this fills from the last frame's filtered tail.
    fn run_prefilter(
        &mut self,
        input: &mut [[f32; MAX_N + OVERLAP]; 2],
        n: usize,
        tapset: usize,
        enabled: bool,
        nb_available_bytes: i32,
    ) -> Prefilter {
        let cc = self.channels;
        let mut pre = [vec![0.0f32; MAX_PERIOD + n], vec![0.0f32; MAX_PERIOD + n]];
        for ch in 0..cc {
            pre[ch][..MAX_PERIOD].copy_from_slice(&self.prefilter_mem[ch]);
            pre[ch][MAX_PERIOD..].copy_from_slice(&input[ch][OVERLAP..OVERLAP + n]);
        }
        let mut pitch = MIN_PERIOD;
        let mut gain1 = 0.0f32;
        if enabled {
            let mut pitch_buf = vec![0.0f32; (MAX_PERIOD + n) >> 1];
            pitch_downsample([&pre[0], &pre[1]], cc, &mut pitch_buf);
            // Not the last 1.5 octaves of the range: short-term correlation gives too many
            // false positives there.
            let found = pitch_search(
                &pitch_buf[MAX_PERIOD >> 1..],
                &pitch_buf,
                n,
                MAX_PERIOD - 3 * MIN_PERIOD,
            );
            pitch = MAX_PERIOD - found.min(MAX_PERIOD - MIN_PERIOD);
            gain1 = remove_doubling(
                &pitch_buf,
                MAX_PERIOD,
                MIN_PERIOD,
                n,
                &mut pitch,
                self.prefilter_period,
                self.prefilter_gain,
            );
            pitch = pitch.min(MAX_PERIOD - 2);
            gain1 *= 0.7;
            if !gain1.is_finite() {
                gain1 = 0.0;
            }
        }
        // A changing pitch and a small frame both ask for a surer one.
        let mut threshold = 0.2f32;
        if (pitch as i32 - self.prefilter_period as i32).abs() * 10 > pitch as i32 {
            threshold += 0.2;
        }
        if nb_available_bytes < 25 {
            threshold += 0.1;
        }
        if nb_available_bytes < 35 {
            threshold += 0.1;
        }
        if self.prefilter_gain > 0.4 {
            threshold -= 0.1;
        }
        if self.prefilter_gain > 0.55 {
            threshold -= 0.1;
        }
        let threshold = threshold.max(0.2);
        let (on, qg);
        if gain1 < threshold {
            gain1 = 0.0;
            on = false;
            qg = 0;
        } else {
            if (gain1 - self.prefilter_gain).abs() < 0.1 {
                gain1 = self.prefilter_gain;
            }
            let q = ((0.5 + gain1 * 32.0 / 3.0).floor() as i32 - 1).clamp(0, 7);
            qg = q as u32;
            gain1 = 0.09375 * (q + 1) as f32;
            on = true;
        }
        self.prefilter_period = self.prefilter_period.max(MIN_PERIOD);
        for ch in 0..cc {
            let buf = &mut input[ch];
            buf[..OVERLAP].copy_from_slice(&self.in_mem[ch]);
            comb_filter(
                &mut buf[OVERLAP..OVERLAP + n],
                &pre[ch],
                MAX_PERIOD,
                self.prefilter_period,
                pitch,
                -self.prefilter_gain,
                -gain1,
                self.prefilter_tapset,
                tapset,
            );
            self.in_mem[ch].copy_from_slice(&buf[n..n + OVERLAP]);
            self.prefilter_mem[ch].copy_from_slice(&pre[ch][n..n + MAX_PERIOD]);
        }
        Prefilter {
            on,
            pitch,
            gain: gain1,
            qg,
        }
    }

    /// An encoder for `channels` (1 or 2) input channels at 48 kHz (`celt_encoder_init`).
    pub(crate) fn new(channels: usize) -> Self {
        Self {
            channels: channels.clamp(1, 2),
            complexity: 10,
            lsb_depth: 24,
            disable_inv: false,
            vbr: None,
            lfe: false,
            rng: 0,
            spread_decision: SPREAD_NORMAL,
            delayed_intra: 1.0,
            tonal_average: 256,
            last_coded_bands: 0,
            hf_average: 0,
            tapset_decision: 0,
            consec_transient: 0,
            preemph_mem: [0.0; 2],
            overlap_max: 0.0,
            stereo_saving: 0.0,
            intensity: 0,
            spec_avg: 0.0,
            in_mem: [[0.0; OVERLAP]; 2],
            prefilter_mem: [vec![0.0; MAX_PERIOD], vec![0.0; MAX_PERIOD]],
            prefilter_period: 0,
            prefilter_gain: 0.0,
            prefilter_tapset: 0,
            old_band_e: [0.0; 2 * NB_EBANDS],
            old_log_e: [-28.0; 2 * NB_EBANDS],
            old_log_e2: [-28.0; 2 * NB_EBANDS],
            energy_error: [0.0; 2 * NB_EBANDS],
        }
    }

    /// Forgets the stream, as `OPUS_RESET_STATE` does; the settings stay.
    pub(crate) fn reset(&mut self) {
        let fresh = Self::new(self.channels);
        *self = Self {
            complexity: self.complexity,
            lsb_depth: self.lsb_depth,
            disable_inv: self.disable_inv,
            vbr: self.vbr,
            lfe: self.lfe,
            ..fresh
        };
    }

    /// `Some(bits per second)`: a variable bitrate around that target (`OPUS_SET_VBR` without
    /// the constraint), each frame shrinking the range encoder it is given (whose size is
    /// then the most it may use) to what it needs. `None`: every byte given is used.
    pub(crate) fn set_vbr(&mut self, bitrate: Option<i32>) {
        self.vbr = bitrate;
    }

    /// Codes the channel as low-frequency effects (`OPUS_SET_LFE`): no transients, no
    /// dynamic allocation, the bands above the second held far below the first.
    pub(crate) fn set_lfe(&mut self, lfe: bool) {
        self.lfe = lfe;
    }

    /// The range after the last frame encoded (`OPUS_GET_FINAL_RANGE`): what a decoder must
    /// end the same frame on.
    pub(crate) fn final_range(&self) -> u32 {
        self.rng
    }

    /// Encodes one frame of `frame.frame_size * channels` interleaved samples (full scale 1.0)
    /// into `renc`, whose size is the frame's (constant bitrate: all of it is used; more than
    /// 1275 bytes are shrunk to that). The caller ends the stream (`RangeEncoder::done`).
    pub(crate) fn encode(
        &mut self,
        pcm: &[f32],
        frame: &CeltFrame,
        renc: &mut RangeEncoder,
    ) -> Result<()> {
        let lm = match frame.frame_size {
            120 => 0usize,
            240 => 1,
            480 => 2,
            960 => 3,
            _ => return Err(Error::BadArgument("a CELT frame is 2.5, 5, 10 or 20 ms")),
        };
        let cc = self.channels;
        let c = frame.stream_channels;
        let (start, end) = (frame.start_band, frame.end_band);
        if start >= NB_EBANDS || !(1..=NB_EBANDS).contains(&end) || start >= end {
            return Err(Error::BadArgument("CELT bands out of range"));
        }
        if !(1..=2).contains(&c) || c > cc {
            return Err(Error::BadArgument("CELT channels out of range"));
        }
        if pcm.len() != frame.frame_size * cc {
            return Err(Error::BadArgument("CELT input of the wrong length"));
        }
        if renc.storage() < 2 {
            return Err(Error::BadArgument("a CELT frame has at least 2 bytes"));
        }
        if renc.storage() > MAX_FRAME_BYTES {
            renc.shrink(MAX_FRAME_BYTES);
        }
        let m = 1usize << lm;
        let n = m * SHORT_MDCT_SIZE;
        let hybrid = start != 0;
        let lmi = lm as i32;
        let ci = c as i32;
        let mut nb_compressed_bytes = renc.storage() as i32;
        let mut enc = Enc::new(renc);

        let mut tell = enc.tell();
        let nb_filled_bytes = (tell + 4) >> 3;
        let mut nb_available_bytes = nb_compressed_bytes - nb_filled_bytes;
        // The target of a variable bitrate, in 1/8 bit a frame.
        let vbr_rate = self.vbr.map_or(0, |bitrate| {
            let den = 48_000 >> BITRES;
            ((i64::from(bitrate) * n as i64 + i64::from(den >> 1)) / i64::from(den)) as i32
        });
        let vbr = vbr_rate > 0;
        let mut effective_bytes = if vbr {
            vbr_rate >> (3 + BITRES)
        } else {
            nb_available_bytes
        };
        let overhead = (40 * ci + 20) * ((400 >> lm) - 50);
        let mut equiv_rate = ((nb_compressed_bytes * 8 * 50) << (3 - lm)) - overhead;
        if let Some(bitrate) = self.vbr {
            equiv_rate = equiv_rate.min(bitrate - overhead);
        }
        let mut total_bits = nb_compressed_bytes * 8;
        let eff_end = end;

        // Samples that are not numbers are silence; nothing passes twice full scale.
        let clean = |v: f32| {
            if v.is_finite() {
                v.clamp(-2.0, 2.0)
            } else {
                0.0
            }
        };
        let peak = |s: &[f32]| s.iter().fold(0.0f32, |a, &v| a.max(clean(v).abs()));
        let split = cc * (n - OVERLAP);
        let mut sample_max = self.overlap_max.max(peak(&pcm[..split]));
        self.overlap_max = peak(&pcm[split..]);
        sample_max = sample_max.max(self.overlap_max);
        let mut silence = sample_max <= 1.0 / (1u32 << self.lsb_depth) as f32;
        if tell == 1 {
            enc.bit_logp(silence, 15);
        } else {
            silence = false;
        }
        if silence {
            // A variable bitrate sends no more than the least there is.
            if vbr {
                nb_compressed_bytes = nb_compressed_bytes.min(nb_filled_bytes + 2);
                effective_bytes = nb_compressed_bytes;
                total_bits = nb_compressed_bytes * 8;
                nb_available_bytes = 2;
                enc.shrink(nb_compressed_bytes as usize);
            }
            // The rest of the frame counts as written: zeros, as the buffer starts.
            tell = nb_compressed_bytes * 8;
            enc.mark_written_to(tell);
        }

        let mut input = [[0.0f32; MAX_N + OVERLAP]; 2];
        for ch in 0..cc {
            let mut mem = self.preemph_mem[ch];
            let buf = &mut input[ch];
            for i in 0..n {
                let x = clean(pcm[cc * i + ch]) * 32768.0;
                buf[OVERLAP + i] = x - mem;
                mem = PREEMPH * x;
            }
            self.preemph_mem[ch] = mem;
        }

        // The pitch pre-filter, where there are bytes for its parameters.
        let prefilter_tapset = self.tapset_decision as usize;
        let enabled = ((self.lfe && nb_available_bytes > 3) || nb_available_bytes > 12 * ci)
            && !hybrid
            && !silence
            && self.complexity >= 5;
        let pf = self.run_prefilter(&mut input, n, prefilter_tapset, enabled, nb_available_bytes);
        let pf_on = pf.on;
        if !pf_on {
            if !hybrid && tell + 16 <= total_bits {
                enc.bit_logp(false, 1);
            }
        } else {
            // No check of the bits left: `enabled` saw to it.
            enc.bit_logp(true, 1);
            let coded = pf.pitch as u32 + 1;
            let octave = (32 - coded.leading_zeros()) - 5;
            enc.uint(octave, 6);
            enc.bits(coded - (16 << octave), 4 + octave);
            enc.bits(pf.qg, 3);
            enc.icdf(prefilter_tapset, &TAPSET_ICDF, 2);
        }

        let mut transient = false;
        let mut short_blocks = false;
        let mut tf_estimate = 0.0f32;
        let mut tf_chan = 0;
        if self.complexity >= 1 && !self.lfe {
            let t = transient_analysis(&input, n + OVERLAP, cc);
            transient = t.is_transient;
            tf_estimate = t.tf_estimate;
            tf_chan = t.tf_chan;
        }
        let mut transient_got_disabled = false;
        if lm > 0 && enc.tell() + 3 <= total_bits {
            short_blocks = transient;
        } else {
            transient = false;
            transient_got_disabled = true;
        }

        let mut freq = [0.0f32; 2 * MAX_N];
        let mut band_e = [0.0f32; 2 * NB_EBANDS];
        let mut band_log_e = [0.0f32; 2 * NB_EBANDS];
        let mut band_log_e2 = [0.0f32; 2 * NB_EBANDS];
        // The long transform's energies too, for the dynamic allocation of a transient.
        let second_mdct = short_blocks && self.complexity >= 8;
        if second_mdct {
            compute_mdcts(false, &input, &mut freq, c, cc, lm);
            compute_band_energies(&freq, &mut band_e, eff_end, c, lm);
            amp2_log2(&band_e, &mut band_log_e2, eff_end, end, c);
            for v in band_log_e2.iter_mut().take(c * NB_EBANDS) {
                *v += 0.5 * lmi as f32;
            }
        }
        compute_mdcts(short_blocks, &input, &mut freq, c, cc, lm);
        if cc == 2 && c == 1 {
            tf_chan = 0;
        }
        compute_band_energies(&freq, &mut band_e, eff_end, c, lm);
        if self.lfe {
            for i in 2..end {
                band_e[i] = band_e[i].min(1e-4 * band_e[0]).max(EPSILON);
            }
        }
        amp2_log2(&band_e, &mut band_log_e, eff_end, end, c);

        // How far the frame stands above the running average of the spectrum (for VBR).
        let temporal_vbr = {
            let mut follow = -10.0f32;
            let mut frame_avg = 0.0f32;
            let offset = if short_blocks { 0.5 * lmi as f32 } else { 0.0 };
            for i in start..end {
                follow = (follow - 1.0).max(band_log_e[i] - offset);
                if c == 2 {
                    follow = follow.max(band_log_e[i + NB_EBANDS] - offset);
                }
                frame_avg += follow;
            }
            frame_avg /= (end - start) as f32;
            let temporal_vbr = (frame_avg - self.spec_avg).clamp(-1.5, 3.0);
            self.spec_avg += 0.02 * temporal_vbr;
            temporal_vbr
        };

        if !second_mdct {
            band_log_e2 = band_log_e;
        }

        // A last chance to catch a transient the time-domain analysis missed.
        if lm > 0
            && enc.tell() + 3 <= total_bits
            && !transient
            && self.complexity >= 5
            && !hybrid
            && !self.lfe
            && patch_transient_decision(&band_log_e, &self.old_band_e, start, end, c)
        {
            transient = true;
            short_blocks = true;
            compute_mdcts(short_blocks, &input, &mut freq, c, cc, lm);
            compute_band_energies(&freq, &mut band_e, eff_end, c, lm);
            amp2_log2(&band_e, &mut band_log_e, eff_end, end, c);
            // Short and long transforms scale differently.
            for v in band_log_e2.iter_mut().take(c * NB_EBANDS) {
                *v += 0.5 * lmi as f32;
            }
            tf_estimate = 0.2;
        }

        if lm > 0 && enc.tell() + 3 <= total_bits {
            enc.bit_logp(transient, 3);
        }

        let mut x = [0.0f32; 2 * MAX_N];
        normalise_bands(&freq, &mut x, &band_e, eff_end, c, lm);

        let enable_tf_analysis =
            effective_bytes >= 15 * ci && !hybrid && self.complexity >= 2 && !self.lfe;
        let mut offsets = [0i32; NB_EBANDS];
        let dynalloc = dynalloc_analysis(
            &band_log_e,
            &band_log_e2,
            start,
            end,
            c,
            &mut offsets,
            self.lsb_depth,
            transient,
            vbr,
            lm,
            // No boosts where the signal stands out for an LFE: its one band gets them all.
            if self.lfe { 0 } else { effective_bytes },
        );
        if self.lfe {
            offsets[0] = 8.min(effective_bytes / 3);
        }

        let mut tf_res = [0i32; NB_EBANDS];
        let tf_select = if enable_tf_analysis {
            let lambda = 80.max(20480 / effective_bytes + 2);
            tf_analysis(
                eff_end,
                transient,
                &mut tf_res,
                lambda,
                &x,
                n,
                lm,
                tf_estimate,
                tf_chan,
                &dynalloc.importance,
            )
        } else {
            for r in tf_res.iter_mut().take(end) {
                *r = i32::from(transient);
            }
            0
        };

        // Stable energy: lean towards the last frame's error, so that the gain does not
        // flutter (a constant offset is better than a changing one).
        for ch in 0..c {
            for i in start..end {
                let at = i + ch * NB_EBANDS;
                if (band_log_e[at] - self.old_band_e[at]).abs() < 2.0 {
                    band_log_e[at] -= self.energy_error[at] * 0.25;
                }
            }
        }
        let mut error = [0.0f32; 2 * NB_EBANDS];
        quant_coarse_energy(
            &CoarseEnergy {
                start,
                end,
                eff_end,
                c,
                lm,
                nb_available_bytes,
                force_intra: false,
                two_pass: self.complexity >= 4,
                loss_rate: 0,
                lfe: self.lfe,
            },
            &band_log_e,
            &mut self.old_band_e,
            total_bits as u32,
            &mut error,
            &mut enc,
            &mut self.delayed_intra,
        );

        tf_encode(start, end, transient, &mut tf_res, lm, tf_select, &mut enc);

        if enc.tell() + 4 <= total_bits {
            if self.lfe {
                self.tapset_decision = 0;
                self.spread_decision = SPREAD_NORMAL;
            } else if hybrid {
                self.spread_decision = if self.complexity == 0 {
                    SPREAD_NONE
                } else if transient {
                    SPREAD_NORMAL
                } else {
                    SPREAD_AGGRESSIVE
                };
            } else if short_blocks || self.complexity < 3 || nb_available_bytes < 10 * ci {
                self.spread_decision = if self.complexity == 0 {
                    SPREAD_NONE
                } else {
                    SPREAD_NORMAL
                };
            } else {
                self.spread_decision = spreading_decision(
                    &x,
                    &mut self.tonal_average,
                    self.spread_decision,
                    &mut self.hf_average,
                    &mut self.tapset_decision,
                    pf_on && !short_blocks,
                    eff_end,
                    c,
                    lm,
                    &dynalloc.spread_weight,
                );
            }
            enc.icdf(self.spread_decision as usize, &SPREAD_ICDF, 5);
        }

        let cap = init_caps(lm, c);
        let mut dynalloc_logp = 6;
        total_bits <<= BITRES;
        let mut total_boost = 0;
        let mut tell_frac = enc.tell_frac();
        for i in start..end {
            let width = (ci * i32::from(EBANDS[i + 1] - EBANDS[i])) << lm;
            // 6 bits a step, but at most 1 bit and at least 1/8 bit per sample.
            let quanta = (width << BITRES).min((6 << BITRES).max(width));
            let mut loop_logp = dynalloc_logp;
            let mut boost = 0;
            let mut j = 0;
            while tell_frac + (loop_logp << BITRES) < total_bits - total_boost && boost < cap[i] {
                let flag = j < offsets[i];
                enc.bit_logp(flag, loop_logp as u32);
                tell_frac = enc.tell_frac();
                if !flag {
                    break;
                }
                boost += quanta;
                total_boost += quanta;
                loop_logp = 1;
                j += 1;
            }
            // A boost makes the next one likelier.
            if j > 0 {
                dynalloc_logp = 2.max(dynalloc_logp - 1);
            }
            offsets[i] = boost;
        }

        let mut dual_stereo = false;
        if c == 2 {
            const INTENSITY_THRESHOLDS: [f32; 21] = [
                1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 16.0, 24.0, 36.0, 44.0, 50.0, 56.0, 62.0,
                67.0, 72.0, 79.0, 88.0, 106.0, 134.0,
            ];
            const INTENSITY_HYSTERESIS: [f32; 21] = [
                1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 3.0, 3.0,
                4.0, 5.0, 6.0, 8.0, 8.0,
            ];
            // Mid/side always at 2.5 ms, until there is a better analysis.
            if lm != 0 {
                dual_stereo = stereo_analysis(&x, lm, n);
            }
            self.intensity = hysteresis_decision(
                (equiv_rate / 1000) as f32,
                &INTENSITY_THRESHOLDS,
                &INTENSITY_HYSTERESIS,
                self.intensity,
            );
            self.intensity = self.intensity.clamp(start, end);
        }

        let mut alloc_trim = 5;
        if tell_frac + (6 << BITRES) <= total_bits - total_boost {
            if start > 0 || self.lfe {
                self.stereo_saving = 0.0;
            } else {
                alloc_trim = alloc_trim_analysis(
                    &x,
                    &band_log_e,
                    end,
                    lm,
                    c,
                    n,
                    &mut self.stereo_saving,
                    tf_estimate,
                    self.intensity,
                    equiv_rate,
                );
            }
            enc.icdf(alloc_trim as usize, &TRIM_ICDF, 7);
            tell_frac = enc.tell_frac();
        }

        if vbr {
            // No more than 510 kb/s even for short frames: the allocator could not use it.
            nb_compressed_bytes = nb_compressed_bytes.min(1275 >> (3 - lm));
            let base_target = vbr_rate - ((40 * ci + 20) << BITRES);
            let target = compute_vbr(
                base_target,
                &VbrInputs {
                    lm,
                    equiv_rate,
                    last_coded_bands: self.last_coded_bands,
                    c,
                    intensity: self.intensity,
                    stereo_saving: self.stereo_saving,
                    tot_boost: dynalloc.tot_boost,
                    tf_estimate,
                    max_depth: dynalloc.max_depth,
                    temporal_vbr,
                },
            ) + tell_frac;
            // Never so small that the bits already promised do not fit; the two bytes of
            // margin keep the decoder's own safeguards from having triggered so far.
            let min_allowed =
                ((tell_frac + total_boost + (1 << (BITRES + 3)) - 1) >> (BITRES + 3)) + 2;
            let mut wanted = (target + (1 << (BITRES + 2))) >> (BITRES + 3);
            wanted = wanted.max(min_allowed).min(nb_compressed_bytes);
            if silence {
                wanted = 2;
            }
            nb_compressed_bytes = nb_compressed_bytes.min(wanted);
            // The raw bits written so far move to the frame's new end.
            enc.shrink(nb_compressed_bytes as usize);
        }

        // The packet, less where we are, less one for safety.
        let mut bits = ((nb_compressed_bytes * 8) << BITRES) - enc.tell_frac() - 1;
        let anti_collapse_rsv = if transient && lm >= 2 && bits >= ((lmi + 2) << BITRES) {
            1 << BITRES
        } else {
            0
        };
        bits -= anti_collapse_rsv;
        let alloc = compute_allocation(
            start,
            end,
            &offsets,
            &cap,
            alloc_trim,
            bits,
            c,
            lm,
            &mut enc,
            &AllocHint {
                intensity: self.intensity,
                dual_stereo,
                prev: self.last_coded_bands,
                signal_bandwidth: if self.lfe { 1 } else { end as i32 - 1 },
            },
        );
        if c == 2 {
            self.intensity = alloc.intensity;
        }
        let coded_bands = alloc.coded_bands as i32;
        self.last_coded_bands = if self.last_coded_bands != 0 {
            (self.last_coded_bands + 1).min((self.last_coded_bands - 1).max(coded_bands))
        } else {
            coded_bands
        };

        quant_fine_energy(
            &mut self.old_band_e,
            &mut error,
            start,
            end,
            &alloc.fine_quant,
            &mut enc,
            c,
        );

        let mut collapse_masks = [0u8; 2 * NB_EBANDS];
        {
            let (xa, ya) = x.split_at_mut(n);
            let all = AllBands {
                start,
                end,
                pulses: &alloc.pulses,
                short_blocks,
                spread: self.spread_decision,
                dual_stereo: alloc.dual_stereo,
                intensity: alloc.intensity,
                tf_res: &tf_res,
                total_bits: nb_compressed_bytes * (8 << BITRES) - anti_collapse_rsv,
                balance: alloc.balance,
                lm,
                coded_bands: alloc.coded_bands,
                band_e: &band_e,
                disable_inv: self.disable_inv,
                theta_rdo: self.complexity >= 8,
            };
            let y = (c == 2).then_some(&mut ya[..n]);
            quant_all_bands(&all, xa, y, &mut collapse_masks, &mut enc, &mut self.rng);
        }

        if anti_collapse_rsv > 0 {
            let anti_collapse_on = self.consec_transient < 2;
            enc.bits(u32::from(anti_collapse_on), 1);
        }
        let bits_left = nb_compressed_bytes * 8 - enc.tell();
        quant_energy_finalise(
            &mut self.old_band_e,
            &mut error,
            start,
            end,
            &alloc.fine_quant,
            &alloc.fine_priority,
            bits_left,
            &mut enc,
            c,
        );
        self.energy_error = [0.0; 2 * NB_EBANDS];
        for ch in 0..c {
            for i in start..end {
                let at = i + ch * NB_EBANDS;
                self.energy_error[at] = error[at].clamp(-0.5, 0.5);
            }
        }
        if silence {
            for v in self.old_band_e.iter_mut().take(c * NB_EBANDS) {
                *v = -28.0;
            }
        }

        if cc == 2 && c == 1 {
            self.old_band_e.copy_within(0..NB_EBANDS, NB_EBANDS);
        }
        if !transient {
            self.old_log_e2 = self.old_log_e;
            self.old_log_e = self.old_band_e;
        } else {
            for (l, &e) in self.old_log_e.iter_mut().zip(&self.old_band_e) {
                *l = l.min(e);
            }
        }
        // In case the bands change with the next frame.
        for ch in 0..2 {
            for i in (0..start).chain(end..NB_EBANDS) {
                self.old_band_e[ch * NB_EBANDS + i] = 0.0;
                self.old_log_e[ch * NB_EBANDS + i] = -28.0;
                self.old_log_e2[ch * NB_EBANDS + i] = -28.0;
            }
        }
        self.prefilter_period = pf.pitch;
        self.prefilter_gain = pf.gain;
        self.prefilter_tapset = prefilter_tapset;
        if transient || transient_got_disabled {
            self.consec_transient += 1;
        } else {
            self.consec_transient = 0;
        }
        self.rng = enc.rng();
        if enc.error() {
            return Err(Error::Corrupt("the CELT frame did not fit its bytes"));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "encoder_tests.rs"]
mod tests;
