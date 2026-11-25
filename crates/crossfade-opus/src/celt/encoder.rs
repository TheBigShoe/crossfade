//! `celt_encode_with_ec` (`celt/celt_encoder.c`, the float build at complexity 9):
//! pre-emphasis, the pitch pre-filter, transient analysis, the MDCTs, band energies and their
//! quantisation, time-frequency resolution, spreading, dynamic allocation, the trim, stereo
//! decisions, the allocator, the band shapes and the anti-collapse flag, under CBR or
//! (constrained) VBR; with the tonality analysis the top level hands it
//! ([`CeltEncoder::set_analysis`]): the pitch gain against the pitch ratio, the leakage
//! boosts, the trim's tonality slope, the VBR target's activity, tonality and pitch change,
//! and the signal's bandwidth.
//!
//! # The contract with the top level
//!
//! The top level owns one [`CeltEncoder`] and calls [`CeltEncoder::encode`] the way
//! `opus_encode_frame_native` calls `celt_encode_with_ec`: with the frame's range encoder,
//! whose [`RangeEncoder::storage`] is the most CELT may use (`nb_compr_bytes`: the top level
//! has shrunk it), and which may already hold SILK's symbols (hybrid: start band 17). CELT
//! may shrink it further (CBR at a bitrate, VBR) and returns the frame's final size; the top
//! level then ends it with [`RangeEncoder::done`]. The final range is the encoder's
//! [`RangeEncoder::rng`] when CELT returns ([`CeltEncoder::final_range`] keeps it).
#![allow(clippy::needless_range_loop)] // index loops as the reference writes them

use super::bands::haar1;
use super::bands_enc::{EncodeBands, quant_all_bands_enc};
use super::maths::exp2;
use super::mdct;
use super::pitch::{pitch_downsample, pitch_search, remove_doubling};
use super::quant_bands::{
    Coarse, amp2_log2, quant_coarse_energy, quant_energy_finalise, quant_fine_energy,
};
use super::rate::{AllocEncoder, BITRES, compute_allocation, init_caps};
use super::tables::{
    COMB_GAINS, E_MEANS, EBANDS, LOG_N, MAX_LM, NB_EBANDS, OVERLAP, PREEMPH, SHORT_MDCT_SIZE,
    SPREAD_ICDF, TAPSET_ICDF, TF_SELECT_TABLE, TRIM_ICDF, WINDOW,
};
use crate::analysis::{AnalysisInfo, LEAK_BANDS};
use crate::error::{Error, Result};
use crate::range_enc::RangeEncoder;

/// The longest pitch period of the pre-filter (`COMBFILTER_MAXPERIOD`).
const COMBFILTER_MAXPERIOD: usize = 1024;
const COMBFILTER_MINPERIOD: i32 = 15;
const SPREAD_NONE: i32 = 0;
const SPREAD_LIGHT: i32 = 1;
const SPREAD_NORMAL: i32 = 2;
const SPREAD_AGGRESSIVE: i32 = 3;
/// The reference encoder's default (`OPUS_SET_COMPLEXITY(9)` in `opus_encoder_init`): every
/// CELT tool is on from 8.
const COMPLEXITY: i32 = 9;
/// The input's depth the float API claims (`lsb_depth`).
const LSB_DEPTH: i32 = 24;
/// Samples by which CELT's output lags its input (`Fs/400`, the overlap).
pub(crate) const DELAY: usize = OVERLAP;
const EPSILON: f32 = 1e-15;

/// What the top level asks of one frame (the `celt_encoder_ctl`s `opus_encode_frame_native`
/// issues before `celt_encode_with_ec`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CeltEncodeFrame {
    /// Samples per channel at 48 kHz: 120, 240, 480 or 960.
    pub(crate) frame_size: usize,
    /// 0, or 17 in hybrid mode.
    pub(crate) start_band: usize,
    /// 13, 17, 19 or 21, from the bandwidth.
    pub(crate) end_band: usize,
    /// Channels coded (`CELT_SET_CHANNELS`): 1 codes a stereo input's downmix.
    pub(crate) stream_channels: usize,
    /// `OPUS_SET_BITRATE`; `None` is `OPUS_BITRATE_MAX` (CBR at the frame's size).
    pub(crate) bitrate: Option<i32>,
    pub(crate) vbr: bool,
    pub(crate) constrained_vbr: bool,
    /// What SILK reported for the frame (`CELT_SET_SILK_INFO`); read in hybrid mode only.
    pub(crate) silk_info: SilkInfo,
}

/// `SILKInfo`: the SILK layer's signal type (2 is voiced) and quantisation offset (Q10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct SilkInfo {
    pub(crate) signal_type: i32,
    pub(crate) offset: i32,
}

/// The CELT encoder's state for one stream (libopus `CELTEncoder`, from `rng` on: what
/// `OPUS_RESET_STATE` clears).
#[derive(Debug, Clone)]
pub(crate) struct CeltEncoder {
    /// Input channels, 1 or 2 (`CC`).
    channels: usize,
    /// `CELT_SET_PREDICTION` below 2: no pitch pre-filter; at 0, intra energy too.
    disable_pf: bool,
    force_intra: bool,
    /// `OPUS_SET_LFE`: a low-frequency effects channel, everything in the first bands.
    lfe: bool,
    // From here on, what OPUS_RESET_STATE clears.
    rng: u32,
    spread_decision: i32,
    delayed_intra: f32,
    tonal_average: i32,
    last_coded_bands: usize,
    hf_average: i32,
    tapset_decision: usize,
    prefilter_period: i32,
    prefilter_gain: f32,
    prefilter_tapset: usize,
    consec_transient: i32,
    /// The tonality analysis of the frame (`CELT_SET_ANALYSIS`; a reset forgets it).
    analysis: AnalysisInfo,
    /// The surround masks of the stream's channels, 21 bands each (`OPUS_SET_ENERGY_MASK`;
    /// a reset forgets them).
    energy_mask: Option<[f32; 2 * NB_EBANDS]>,
    preemph_mem: [f32; 2],
    vbr_reservoir: i32,
    vbr_drift: i32,
    vbr_offset: i32,
    vbr_count: i32,
    overlap_max: f32,
    stereo_saving: f32,
    intensity: usize,
    spec_avg: f32,
    /// Per channel, the last `OVERLAP` pre-filtered samples (`in_mem`).
    in_mem: [[f32; OVERLAP]; 2],
    /// Per channel, the last `COMBFILTER_MAXPERIOD` unfiltered samples (`prefilter_mem`).
    prefilter_mem: [[f32; COMBFILTER_MAXPERIOD]; 2],
    old_band_e: [f32; 2 * NB_EBANDS],
    old_log_e: [f32; 2 * NB_EBANDS],
    old_log_e2: [f32; 2 * NB_EBANDS],
    energy_error: [f32; 2 * NB_EBANDS],
}

fn eb(i: usize) -> usize {
    EBANDS[i] as usize
}

/// `celt_maxabs16`.
fn maxabs(x: &[f32]) -> f32 {
    let mut maxval = 0.0f32;
    let mut minval = 0.0f32;
    for &v in x {
        maxval = maxval.max(v);
        minval = minval.min(v);
    }
    maxval.max(-minval)
}

/// `hysteresis_decision`.
fn hysteresis_decision(val: f32, thresholds: &[f32], hysteresis: &[f32], prev: usize) -> usize {
    let n = thresholds.len();
    let mut i = 0;
    while i < n && val >= thresholds[i] {
        i += 1;
    }
    if i > prev && val < thresholds[prev] + hysteresis[prev] {
        i = prev;
    }
    if i < prev && val > thresholds[prev - 1] - hysteresis[prev - 1] {
        i = prev;
    }
    i
}

/// `comb_filter` from `x` (read from `x_at`, with up to `t + 2` samples of history before it)
/// into `y`: the (pre-)filter cross-faded from (`t0`, `g0`, `tapset0`) to (`t1`, `g1`,
/// `tapset1`) over the overlap.
#[allow(clippy::too_many_arguments)]
fn comb_filter(
    y: &mut [f32],
    x: &[f32],
    x_at: usize,
    t0: i32,
    t1: i32,
    n: usize,
    g0: f32,
    g1: f32,
    tapset0: usize,
    tapset1: usize,
) {
    let y = &mut y[..n];
    if g0 == 0.0 && g1 == 0.0 {
        y.copy_from_slice(&x[x_at..x_at + n]);
        return;
    }
    // A zero gain may come with a zero period: at least the minimum keeps reads in range.
    let t0 = t0.max(COMBFILTER_MINPERIOD) as usize;
    let t1 = t1.max(COMBFILTER_MINPERIOD) as usize;
    let g00 = g0 * COMB_GAINS[tapset0][0];
    let g01 = g0 * COMB_GAINS[tapset0][1];
    let g02 = g0 * COMB_GAINS[tapset0][2];
    let g10 = g1 * COMB_GAINS[tapset1][0];
    let g11 = g1 * COMB_GAINS[tapset1][1];
    let g12 = g1 * COMB_GAINS[tapset1][2];
    let mut overlap = OVERLAP.min(n);
    // An unchanged filter needs no cross-fade.
    if g0 == g1 && t0 == t1 && tapset0 == tapset1 {
        overlap = 0;
    }
    for i in 0..overlap {
        let at = x_at + i;
        let f = WINDOW[i] * WINDOW[i];
        let nf = 1.0 - f;
        y[i] = x[at]
            + (nf * g00) * x[at - t0]
            + (nf * g01) * (x[at - t0 + 1] + x[at - t0 - 1])
            + (nf * g02) * (x[at - t0 + 2] + x[at - t0 - 2])
            + (f * g10) * x[at - t1]
            + (f * g11) * (x[at - t1 + 1] + x[at - t1 - 1])
            + (f * g12) * (x[at - t1 + 2] + x[at - t1 - 2]);
    }
    if g1 == 0.0 {
        y[overlap..].copy_from_slice(&x[x_at + overlap..x_at + n]);
        return;
    }
    for i in overlap..n {
        let at = x_at + i;
        y[i] = x[at]
            + g10 * x[at - t1]
            + g11 * (x[at - t1 + 1] + x[at - t1 - 1])
            + g12 * (x[at - t1 + 2] + x[at - t1 - 2]);
    }
}

/// `transient_analysis`: whether the frame (`len` samples per channel, channel `c` at
/// `c * len`) needs short blocks, with the estimate of how strongly (`tf_estimate`) and the
/// channel that decided it.
/// Returns whether the frame is transient, `tf_estimate`, the channel that decided and, with
/// `allow_weak_transients`, whether it is a weak transient coded as a long block.
fn transient_analysis(
    input: &[f32],
    len: usize,
    channels: usize,
    allow_weak_transients: bool,
) -> (bool, f32, usize, bool) {
    // 6 * 64 / x, trained on real data to minimise the average error.
    const INV_TABLE: [u8; 128] = [
        255, 255, 156, 110, 86, 70, 59, 51, 45, 40, 37, 33, 31, 28, 26, 25, 23, 22, 21, 20, 19, 18,
        17, 16, 16, 15, 15, 14, 13, 13, 12, 12, 12, 12, 11, 11, 11, 10, 10, 10, 9, 9, 9, 9, 9, 9,
        8, 8, 8, 8, 8, 7, 7, 7, 7, 7, 7, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 5, 5, 5,
        5, 5, 5, 5, 5, 5, 5, 5, 5, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
        4, 4, 4, 4, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 2,
    ];
    // Forward masking: 6.7 dB/ms; 3.3 dB/ms at low rates, so as not to code transients that
    // would leave the energy unstable or partly collapsed (mostly hybrid).
    let forward_decay = if allow_weak_transients {
        0.03125
    } else {
        0.0625f32
    };
    let len2 = len / 2;
    let mut tmp = vec![0.0f32; len];
    let mut mask_metric = 0i32;
    let mut tf_chan = 0;
    for c in 0..channels {
        let x = &input[c * len..(c + 1) * len];
        let mut mem0 = 0.0f32;
        let mut mem1 = 0.0f32;
        // High-pass: (1 - 2/z + 1/z^2) / (1 - 1/z + 1/(2 z^2)).
        for i in 0..len {
            let xi = x[i];
            let y = mem0 + xi;
            let mem00 = mem0;
            mem0 = mem0 - xi + 0.5 * mem1;
            mem1 = xi - mem00;
            tmp[i] = y;
        }
        // The first samples are bad: the memory is not carried over.
        tmp[..12].fill(0.0);
        let mut mean = 0.0f32;
        let mut mem0 = 0.0f32;
        // Forward pass for the post-echo threshold, two samples at a time.
        for i in 0..len2 {
            let x2 = tmp[2 * i] * tmp[2 * i] + tmp[2 * i + 1] * tmp[2 * i + 1];
            mean += x2;
            mem0 = x2 + (1.0 - forward_decay) * mem0;
            tmp[i] = forward_decay * mem0;
        }
        let mut mem0 = 0.0f32;
        let mut max_e = 0.0f32;
        // Backward pass for the pre-echo threshold: 13.9 dB/ms.
        for i in (0..len2).rev() {
            mem0 = tmp[i] + 0.875 * mem0;
            tmp[i] = 0.125 * mem0;
            max_e = max_e.max(0.125 * mem0);
        }
        // The frame energy: the geometric mean of the energy and half the maximum.
        let mean = (f64::from(mean * max_e) * 0.5 * len2 as f64).sqrt() as f32;
        let norm = len2 as f32 / (EPSILON + mean);
        let mut unmask = 0i32;
        let mut i = 12;
        while i + 5 < len2 {
            let v = libm::floorf(64.0 * norm * (tmp[i] + EPSILON));
            // NaN would mean something went badly wrong upstream; 0 keeps the read in range.
            let id = if v >= 127.0 {
                127
            } else if v > 0.0 {
                v as usize
            } else {
                0
            };
            unmask += i32::from(INV_TABLE[id]);
            i += 4;
        }
        // Compensate for the quarter of the samples and the factor 6 of the table.
        let unmask = 64 * unmask * 4 / (6 * (len2 as i32 - 17));
        if unmask > mask_metric {
            tf_chan = c;
            mask_metric = unmask;
        }
    }
    let mut is_transient = mask_metric > 200;
    // At low rates a weak transient is coded with a long block and TF instead.
    let weak = allow_weak_transients && is_transient && mask_metric < 600;
    if weak {
        is_transient = false;
    }
    let tf_max = ((27.0 * f64::from(mask_metric)).sqrt() as f32 - 42.0).max(0.0);
    let tf_estimate = (0.0069 * tf_max.min(163.0) - 0.139).max(0.0).sqrt();
    (is_transient, tf_estimate, tf_chan, weak)
}

/// `patch_transient_decision`: a sudden rise of energy the time analysis missed.
fn patch_transient_decision(
    new_e: &[f32; 2 * NB_EBANDS],
    old_e: &[f32; 2 * NB_EBANDS],
    start: usize,
    end: usize,
    c: usize,
) -> bool {
    let mut spread_old = [0.0f32; 26];
    // An aggressive -6 dB/Bark spreading of the old frame, against false detection.
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
    for i in (start..end - 1).rev() {
        spread_old[i] = spread_old[i].max(spread_old[i + 1] - 1.0);
    }
    let mut mean_diff = 0.0f32;
    let first = start.max(2);
    for ch in 0..c {
        for i in first..end - 1 {
            let x1 = new_e[i + ch * NB_EBANDS].max(0.0);
            let x2 = spread_old[i].max(0.0);
            mean_diff += (x1 - x2).max(0.0);
        }
    }
    let count = (c * (end - 1 - first)) as f32;
    mean_diff / count > 1.0
}

/// `compute_mdcts`: the windowed MDCTs of every channel (`in` holds `blocks * n + OVERLAP`
/// samples per channel), interleaved per block; a stereo input coded as mono is averaged.
fn compute_mdcts(
    input: &[f32],
    out: &mut [f32],
    short_blocks: bool,
    c: usize,
    cc: usize,
    lm: usize,
) {
    let (blocks, n, shift) = if short_blocks {
        (1 << lm, SHORT_MDCT_SIZE, MAX_LM)
    } else {
        (1, SHORT_MDCT_SIZE << lm, MAX_LM - lm)
    };
    let len = blocks * n + OVERLAP;
    for ch in 0..cc {
        for b in 0..blocks {
            let x = &input[ch * len + b * n..ch * len + b * n + n + OVERLAP];
            mdct::forward(x, &mut out[b + ch * n * blocks..], blocks, shift);
        }
    }
    if cc == 2 && c == 1 {
        let total = blocks * n;
        for i in 0..total {
            out[i] = 0.5 * out[i] + 0.5 * out[total + i];
        }
    }
}

/// `compute_band_energies`: each band's amplitude.
pub(super) fn compute_band_energies(
    freq: &[f32],
    band_e: &mut [f32; 2 * NB_EBANDS],
    end: usize,
    c: usize,
    lm: usize,
) {
    let n = SHORT_MDCT_SIZE << lm;
    for ch in 0..c {
        for i in 0..end {
            let mut sum = 0.0f32;
            for &v in &freq[ch * n + (eb(i) << lm)..ch * n + (eb(i + 1) << lm)] {
                sum += v * v;
            }
            band_e[i + ch * NB_EBANDS] = (1e-27 + sum).sqrt();
        }
    }
}

/// `normalise_bands`: each band to unit energy.
fn normalise_bands(
    freq: &[f32],
    x: &mut [f32],
    band_e: &[f32; 2 * NB_EBANDS],
    end: usize,
    c: usize,
    m: usize,
) {
    let n = m * SHORT_MDCT_SIZE;
    for ch in 0..c {
        for i in 0..end {
            let g = 1.0 / (1e-27 + band_e[i + ch * NB_EBANDS]);
            for j in m * eb(i)..m * eb(i + 1) {
                x[j + ch * n] = freq[j + ch * n] * g;
            }
        }
    }
}

/// `l1_metric`: the L1 norm, biased towards frequency resolution by `lm * bias`.
fn l1_metric(tmp: &[f32], lm: i32, bias: f32) -> f32 {
    let mut l1 = 0.0f32;
    for &v in tmp {
        l1 += v.abs();
    }
    l1 + (lm as f32 * bias) * l1
}

/// `tf_analysis`: the time-frequency resolution change per band (Viterbi over the L1 cost of
/// each), and `tf_select`.
#[allow(clippy::too_many_arguments)]
fn tf_analysis(
    len: usize,
    is_transient: bool,
    tf_res: &mut [i32; NB_EBANDS],
    lambda: i32,
    x: &[f32],
    n0: usize,
    lm: usize,
    tf_estimate: f32,
    tf_chan: usize,
    importance: &[i32; NB_EBANDS],
) -> usize {
    let lmi = lm as i32;
    let bias = 0.04 * (-0.25f32).max(0.5 - tf_estimate);
    let mut metric = [0i32; NB_EBANDS];
    let mut tmp = [0.0f32; 176];
    let mut tmp_1 = [0.0f32; 176];
    let t = usize::from(is_transient);
    for i in 0..len {
        let n = (eb(i + 1) - eb(i)) << lm;
        // A band too narrow to split down to LM = -1.
        let narrow = eb(i + 1) - eb(i) == 1;
        let at = tf_chan * n0 + (eb(i) << lm);
        tmp[..n].copy_from_slice(&x[at..at + n]);
        let mut best_l1 = l1_metric(&tmp[..n], if is_transient { lmi } else { 0 }, bias);
        let mut best_level = 0;
        if is_transient && !narrow {
            tmp_1[..n].copy_from_slice(&tmp[..n]);
            haar1(&mut tmp_1[..n], n >> lm, 1 << lm);
            let l1 = l1_metric(&tmp_1[..n], lmi + 1, bias);
            if l1 < best_l1 {
                best_l1 = l1;
                best_level = -1;
            }
        }
        let levels = lm + usize::from(!(is_transient || narrow));
        for k in 0..levels {
            let b = if is_transient {
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
        // In Q1, to pick the half-way point for the narrow bands.
        metric[i] = if is_transient {
            2 * best_level
        } else {
            -2 * best_level
        };
        if narrow && (metric[i] == 0 || metric[i] == -2 * lmi) {
            metric[i] -= 1;
        }
    }
    let table = |sel: usize, k: usize| 2 * i32::from(TF_SELECT_TABLE[lm][4 * t + 2 * sel + k]);
    let first_cost1 = |sel| {
        let bias = if is_transient { 0 } else { lambda };
        importance[0] * (metric[0] - table(sel, 1)).abs() + bias
    };
    let mut selcost = [0i32; 2];
    for sel in 0..2 {
        let mut cost0 = importance[0] * (metric[0] - table(sel, 0)).abs();
        let mut cost1 = first_cost1(sel);
        for i in 1..len {
            let curr0 = cost0.min(cost1 + lambda);
            let curr1 = (cost0 + lambda).min(cost1);
            cost0 = curr0 + importance[i] * (metric[i] - table(sel, 0)).abs();
            cost1 = curr1 + importance[i] * (metric[i] - table(sel, 1)).abs();
        }
        selcost[sel] = cost0.min(cost1);
    }
    // tf_select = 1 only for transients, for now (as the reference).
    let tf_select = usize::from(selcost[1] < selcost[0] && is_transient);
    let mut path0 = [0i32; NB_EBANDS];
    let mut path1 = [0i32; NB_EBANDS];
    let mut cost0 = importance[0] * (metric[0] - table(tf_select, 0)).abs();
    let mut cost1 = first_cost1(tf_select);
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
        cost0 = curr0 + importance[i] * (metric[i] - table(tf_select, 0)).abs();
        cost1 = curr1 + importance[i] * (metric[i] - table(tf_select, 1)).abs();
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

/// `tf_encode`: codes the per-band changes and `tf_select`, and turns `tf_res` into the
/// resolution changes the bands use.
fn tf_encode(
    start: usize,
    end: usize,
    is_transient: bool,
    tf_res: &mut [i32; NB_EBANDS],
    lm: usize,
    mut tf_select: usize,
    enc: &mut RangeEncoder,
) {
    let mut budget = (enc.storage() * 8) as u32;
    let mut tell = enc.tell() as u32;
    let mut logp: u32 = if is_transient { 2 } else { 4 };
    // Room for the tf_select decision.
    let tf_select_rsv = lm > 0 && tell + logp < budget;
    budget -= u32::from(tf_select_rsv);
    let mut curr = 0;
    let mut tf_changed = 0;
    for i in start..end {
        if tell + logp <= budget {
            enc.enc_bit_logp(tf_res[i] ^ curr != 0, logp);
            tell = enc.tell() as u32;
            curr = tf_res[i];
            tf_changed |= curr;
        } else {
            tf_res[i] = curr;
        }
        logp = if is_transient { 4 } else { 5 };
    }
    let t = 4 * usize::from(is_transient);
    // Only coded where it makes a difference.
    if tf_select_rsv
        && TF_SELECT_TABLE[lm][t + tf_changed as usize]
            != TF_SELECT_TABLE[lm][t + 2 + tf_changed as usize]
    {
        enc.enc_bit_logp(tf_select != 0, 1);
    } else {
        tf_select = 0;
    }
    for r in tf_res.iter_mut().take(end).skip(start) {
        *r = i32::from(TF_SELECT_TABLE[lm][t + 2 * tf_select + *r as usize]);
    }
}

/// `median_of_5`.
fn median_of_5(x: &[f32]) -> f32 {
    let t2 = x[2];
    let (mut t0, mut t1) = if x[0] > x[1] {
        (x[1], x[0])
    } else {
        (x[0], x[1])
    };
    let (mut t3, mut t4) = if x[3] > x[4] {
        (x[4], x[3])
    } else {
        (x[3], x[4])
    };
    if t0 > t3 {
        std::mem::swap(&mut t0, &mut t3);
        std::mem::swap(&mut t1, &mut t4);
    }
    if t2 > t1 {
        if t1 < t3 { t2.min(t3) } else { t4.min(t1) }
    } else if t2 < t3 {
        t1.min(t3)
    } else {
        t2.min(t4)
    }
}

/// `median_of_3`.
fn median_of_3(x: &[f32]) -> f32 {
    let (t0, t1) = if x[0] > x[1] {
        (x[1], x[0])
    } else {
        (x[0], x[1])
    };
    let t2 = x[2];
    if t1 < t2 {
        t1
    } else if t0 < t2 {
        t2
    } else {
        t0
    }
}

/// What [`dynalloc_analysis`] finds.
struct Dynalloc {
    offsets: [i32; NB_EBANDS],
    importance: [i32; NB_EBANDS],
    spread_weight: [i32; NB_EBANDS],
    tot_boost: i32,
    max_depth: f32,
}

/// `dynalloc_analysis`: boosts for the bands that stand out of a smoothed spectrum, the
/// bands' importance for the TF search, and the spreading decision's weights.
#[allow(clippy::too_many_arguments)]
fn dynalloc_analysis(
    band_log_e: &[f32; 2 * NB_EBANDS],
    band_log_e2: &[f32; 2 * NB_EBANDS],
    old_band_e: &[f32; 2 * NB_EBANDS],
    start: usize,
    end: usize,
    c: usize,
    is_transient: bool,
    vbr: bool,
    constrained_vbr: bool,
    lm: usize,
    effective_bytes: i32,
    lfe: bool,
    surround_dynalloc: &[f32; NB_EBANDS],
    analysis: &AnalysisInfo,
) -> Dynalloc {
    let mut d = Dynalloc {
        offsets: [0; NB_EBANDS],
        importance: [0; NB_EBANDS],
        spread_weight: [0; NB_EBANDS],
        tot_boost: 0,
        max_depth: -31.9,
    };
    let mut noise_floor = [0.0f32; NB_EBANDS];
    for i in 0..end {
        // The depth, the band's width, its mean and the pre-emphasis (about the square of
        // the Bark band).
        noise_floor[i] = 0.0625 * f32::from(LOG_N[i]) + 0.5 + (9 - LSB_DEPTH) as f32 - E_MEANS[i]
            + 0.0062 * ((i + 5) * (i + 5)) as f32;
    }
    for ch in 0..c {
        for i in 0..end {
            d.max_depth = d
                .max_depth
                .max(band_log_e[ch * NB_EBANDS + i] - noise_floor[i]);
        }
    }
    {
        // A crude masking model, so that masked bands do not sway the spreading decision.
        let mut mask = [0.0f32; NB_EBANDS];
        for i in 0..end {
            mask[i] = band_log_e[i] - noise_floor[i];
        }
        if c == 2 {
            for i in 0..end {
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
            // The mask is never more than 72 dB below the peak, nor below the noise floor.
            let smr = sig[i] - (d.max_depth - 12.0).max(0.0).max(mask[i]);
            let shift = (-(libm::floorf(0.5 + smr) as i32)).clamp(0, 5);
            d.spread_weight[i] = 32 >> shift;
        }
    }
    // From 24 kb/s at 20 ms (96 at 2.5 ms): dynalloc can then not bust the budget.
    if effective_bytes >= 30 + 5 * lm as i32 && !lfe {
        let mut follower = [0.0f32; 2 * NB_EBANDS];
        let mut last = 0;
        for ch in 0..c {
            let mut e3 = [0.0f32; NB_EBANDS];
            e3[..end].copy_from_slice(&band_log_e2[ch * NB_EBANDS..ch * NB_EBANDS + end]);
            if lm == 0 {
                // One bin per band in the first 8 at 2.5 ms: too noisy alone.
                for i in 0..8.min(end) {
                    e3[i] = band_log_e2[ch * NB_EBANDS + i].max(old_band_e[ch * NB_EBANDS + i]);
                }
            }
            let f = &mut follower[ch * NB_EBANDS..ch * NB_EBANDS + NB_EBANDS];
            f[0] = e3[0];
            for i in 1..end {
                // The last band at least 3 dB above the one before is the last considered:
                // band-limited signals otherwise suffer.
                if e3[i] > e3[i - 1] + 0.5 {
                    last = i;
                }
                f[i] = (f[i - 1] + 1.5).min(e3[i]);
            }
            for i in (0..last).rev() {
                f[i] = f[i].min((f[i + 1] + 2.0).min(e3[i]));
            }
            // With a median filter, so that dynalloc does not trigger needlessly.
            let offset = 1.0f32;
            for i in 2..end.saturating_sub(2) {
                f[i] = f[i].max(median_of_5(&e3[i - 2..]) - offset);
            }
            let tmp = median_of_3(&e3[0..]) - offset;
            f[0] = f[0].max(tmp);
            f[1] = f[1].max(tmp);
            let tmp = median_of_3(&e3[end - 3..]) - offset;
            f[end - 2] = f[end - 2].max(tmp);
            f[end - 1] = f[end - 1].max(tmp);
            for i in 0..end {
                f[i] = f[i].max(noise_floor[i]);
            }
        }
        if c == 2 {
            for i in start..end {
                // 24 dB of cross-talk.
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
            follower[i] = follower[i].max(surround_dynalloc[i]);
        }
        for i in start..end {
            d.importance[i] = libm::floorf(0.5 + 13.0 * exp2(follower[i].min(4.0))) as i32;
        }
        // CBR and constrained VBR halve dynalloc on frames without transients.
        if (!vbr || constrained_vbr) && !is_transient {
            for f in &mut follower[start..end] {
                *f *= 0.5;
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
        if analysis.valid {
            for i in start..end.min(LEAK_BANDS) {
                follower[i] += (1.0 / 64.0) * f32::from(analysis.leak_boost[i]);
            }
        }
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
            // CBR and constrained VBR without transients: at most 2/3 of the bits.
            if (!vbr || (constrained_vbr && !is_transient))
                && (d.tot_boost + boost_bits) >> BITRES >> 3 > 2 * effective_bytes / 3
            {
                let cap = (2 * effective_bytes / 3) << BITRES << 3;
                d.offsets[i] = cap - d.tot_boost;
                d.tot_boost = cap;
                break;
            }
            d.offsets[i] = boost;
            d.tot_boost += boost_bits;
        }
    } else {
        for i in start..end {
            d.importance[i] = 13;
        }
    }
    d
}

/// `spreading_decision`: how much to spread the pulses, from how peaky the bands are; also
/// updates the post-filter's tapset decision (`update_hf`).
#[allow(clippy::too_many_arguments)]
fn spreading_decision(
    x: &[f32],
    average: &mut i32,
    last_decision: i32,
    hf_average: &mut i32,
    tapset_decision: &mut usize,
    update_hf: bool,
    end: usize,
    c: usize,
    m: usize,
    spread_weight: &[i32; NB_EBANDS],
) -> i32 {
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
            let xb = &x[m * eb(i) + ch * n0..m * eb(i) + ch * n0 + n];
            let mut tcount = [0i32; 3];
            // A rough CDF of |x[j]|.
            for &v in xb {
                let x2n = (v * v) * n as f32;
                if x2n < 0.25 {
                    tcount[0] += 1;
                }
                if x2n < 0.0625 {
                    tcount[1] += 1;
                }
                if x2n < 0.015625 {
                    tcount[2] += 1;
                }
            }
            // Only the last four bands (8 kHz and up).
            if i > NB_EBANDS - 4 {
                hf_sum += 32 * (tcount[1] + tcount[0]) / n as i32;
            }
            let ni = n as i32;
            let tmp = i32::from(2 * tcount[2] >= ni)
                + i32::from(2 * tcount[1] >= ni)
                + i32::from(2 * tcount[0] >= ni);
            sum += tmp * spread_weight[i];
            nb_bands += spread_weight[i];
        }
    }
    if update_hf {
        if hf_sum != 0 {
            hf_sum /= (c * (4 + end - NB_EBANDS)) as i32;
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
    let mut sum = (sum << 8) / nb_bands.max(1);
    // Recursive averaging, then hysteresis.
    sum = (sum + *average) >> 1;
    *average = sum;
    sum = (3 * sum + (((3 - last_decision) << 7) + 64) + 2) >> 2;
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

/// `alloc_trim_analysis`: the allocation's tilt, from the spectral tilt, the transients and
/// the inter-channel correlation (which also updates `stereo_saving`).
#[allow(clippy::too_many_arguments)]
fn alloc_trim_analysis(
    x: &[f32],
    band_log_e: &[f32; 2 * NB_EBANDS],
    end: usize,
    lm: usize,
    c: usize,
    n0: usize,
    stereo_saving: &mut f32,
    tf_estimate: f32,
    intensity: usize,
    equiv_rate: i32,
    surround_trim: f32,
    analysis: &AnalysisInfo,
) -> i32 {
    // At low rates a lower trim seems to help; above, it is kept as it was.
    let mut trim = 5.0f32;
    if equiv_rate < 64_000 {
        trim = 4.0;
    } else if equiv_rate < 80_000 {
        let frac = (equiv_rate - 64_000) >> 10;
        trim = 4.0 + (1.0 / 16.0) * frac as f32;
    }
    if c == 2 {
        let partial = |i: usize| -> f32 {
            let lo = eb(i) << lm;
            let n = (eb(i + 1) - eb(i)) << lm;
            let mut s = 0.0f32;
            for j in 0..n {
                s += x[lo + j] * x[n0 + lo + j];
            }
            s
        };
        // The inter-channel correlation of the low bands.
        let mut sum = 0.0f32;
        for i in 0..8 {
            sum += partial(i);
        }
        sum *= 1.0 / 8.0;
        sum = sum.abs().min(1.0);
        let mut min_xc = sum;
        for i in 8..intensity {
            min_xc = min_xc.min(partial(i).abs());
        }
        min_xc = min_xc.abs().min(1.0);
        // Mid/side savings from the low-band average and from the least correlated band.
        let log_xc = super::maths::log2(1.001 - sum * sum);
        let log_xc2 = (0.5 * log_xc).max(super::maths::log2(1.001 - min_xc * min_xc));
        trim += (-4.0f32).max(0.75 * log_xc);
        *stereo_saving = (*stereo_saving + 0.25).min(-0.5 * log_xc2);
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
    trim -= surround_trim;
    trim -= 2.0 * tf_estimate;
    if analysis.valid {
        trim -= (2.0 * (analysis.tonality_slope + 0.05)).clamp(-2.0, 2.0);
    }
    (libm::floorf(0.5 + trim) as i32).clamp(0, 10)
}

#[allow(clippy::approx_constant)] // the reference's 0.707107, not 1/sqrt(2)
/// `stereo_analysis`: whether left/right (dual stereo) codes cheaper than mid/side, by the
/// L1 norms of the low bands.
fn stereo_analysis(x: &[f32], lm: usize, n0: usize) -> bool {
    let mut sum_lr = EPSILON;
    let mut sum_ms = EPSILON;
    for i in 0..13 {
        for j in eb(i) << lm..eb(i + 1) << lm {
            let l = x[j];
            let r = x[n0 + j];
            let m = l + r;
            let s = l - r;
            sum_lr += l.abs() + r.abs();
            sum_ms += m.abs() + s.abs();
        }
    }
    sum_ms *= 0.707_107;
    // The thetas of the low bands are not needed at LM 0 and 1.
    let thetas = if lm <= 1 { 13 - 8 } else { 13 };
    let base = eb(13) << (lm + 1);
    (base + thetas) as f32 * sum_ms > base as f32 * sum_lr
}

/// `compute_vbr`: the frame's target in 1/8 bit.
#[allow(clippy::too_many_arguments)]
fn compute_vbr(
    analysis: &AnalysisInfo,
    base_target: i32,
    lm: usize,
    bitrate: i32,
    last_coded_bands: usize,
    c: usize,
    intensity: usize,
    constrained_vbr: bool,
    stereo_saving: f32,
    tot_boost: i32,
    tf_estimate: f32,
    max_depth: f32,
    temporal_vbr: f32,
    pitch_change: bool,
    lfe: bool,
    surround_masking: Option<f32>,
) -> i32 {
    let coded_bands = if last_coded_bands != 0 {
        last_coded_bands
    } else {
        NB_EBANDS
    };
    let mut coded_bins = (eb(coded_bands) << lm) as i32;
    if c == 2 {
        coded_bins += (eb(intensity.min(coded_bands)) << lm) as i32;
    }
    let mut target = base_target;
    if analysis.valid && f64::from(analysis.activity) < 0.4 {
        target -= (((coded_bins << BITRES) as f32) * (0.4 - analysis.activity)) as i32;
    }
    if c == 2 {
        // The most a mono signal could save.
        let coded_stereo_bands = intensity.min(coded_bands);
        let dof = (eb(coded_stereo_bands) << lm) as i32 - coded_stereo_bands as i32;
        let max_frac = (0.8 * dof as f32) / coded_bins as f32;
        let stereo_saving = stereo_saving.min(1.0);
        target -=
            (max_frac * target as f32).min((stereo_saving - 0.1) * (dof << BITRES) as f32) as i32;
    }
    // The dynalloc boost, less its average.
    target += tot_boost - (19 << lm);
    // The transient boost, less its average.
    let tf_calibration = 0.044f32;
    target += ((tf_estimate - tf_calibration) * target as f32) as i32;
    if analysis.valid && !lfe {
        // The tonality boost, less its average.
        let tonal = (analysis.tonality - 0.15).max(0.0) - 0.12;
        let mut tonal_target = target + (((coded_bins << BITRES) as f32) * 1.2 * tonal) as i32;
        if pitch_change {
            tonal_target += (((coded_bins << BITRES) as f32) * 0.8) as i32;
        }
        target = tonal_target;
    }
    if let (Some(masking), false) = (surround_masking, lfe) {
        let surround_target = target + (masking * (coded_bins << BITRES) as f32) as i32;
        target = (target / 4).max(surround_target);
    }
    {
        let bins = (eb(NB_EBANDS - 2) << lm) as i32;
        let floor_depth = ((((c as i32 * bins) << BITRES) as f32) * max_depth) as i32;
        let floor_depth = floor_depth.max(target >> 2);
        target = target.min(floor_depth);
    }
    // Constrained VBR cannot keep a higher rate long: less aggressive.
    if (surround_masking.is_none() || lfe) && constrained_vbr {
        target = base_target + (0.67 * (target - base_target) as f32) as i32;
    }
    if surround_masking.is_none() && tf_estimate < 0.2 {
        let amount = 0.000_003_1 * (96_000 - bitrate).clamp(0, 32_000) as f32;
        let tvbr_factor = temporal_vbr * amount;
        target += (tvbr_factor * target as f32) as i32;
    }
    // At most double the rate.
    target.min(2 * base_target)
}

impl CeltEncoder {
    /// An encoder of `channels` (1 or 2) input channels at 48 kHz (`celt_encoder_init`).
    pub(crate) fn new(channels: usize) -> Self {
        Self {
            channels: channels.clamp(1, 2),
            disable_pf: false,
            force_intra: false,
            lfe: false,
            rng: 0,
            spread_decision: SPREAD_NORMAL,
            delayed_intra: 1.0,
            tonal_average: 256,
            last_coded_bands: 0,
            hf_average: 0,
            tapset_decision: 0,
            prefilter_period: 0,
            prefilter_gain: 0.0,
            prefilter_tapset: 0,
            consec_transient: 0,
            analysis: AnalysisInfo::default(),
            energy_mask: None,
            preemph_mem: [0.0; 2],
            vbr_reservoir: 0,
            vbr_drift: 0,
            vbr_offset: 0,
            vbr_count: 0,
            overlap_max: 0.0,
            stereo_saving: 0.0,
            intensity: 0,
            spec_avg: 0.0,
            in_mem: [[0.0; OVERLAP]; 2],
            prefilter_mem: [[0.0; COMBFILTER_MAXPERIOD]; 2],
            old_band_e: [0.0; 2 * NB_EBANDS],
            old_log_e: [-28.0; 2 * NB_EBANDS],
            old_log_e2: [-28.0; 2 * NB_EBANDS],
            energy_error: [0.0; 2 * NB_EBANDS],
        }
    }

    /// Forgets the stream (`OPUS_RESET_STATE`).
    pub(crate) fn reset(&mut self) {
        *self = Self {
            disable_pf: self.disable_pf,
            force_intra: self.force_intra,
            lfe: self.lfe,
            ..Self::new(self.channels)
        };
    }

    /// `CELT_SET_PREDICTION`: 2 (the default) predicts freely; 1 turns the pitch pre-filter
    /// off; 0 also codes every frame's energy intra, so that each frame decodes alone.
    pub(crate) fn set_prediction(&mut self, prediction: u8) {
        self.disable_pf = prediction <= 1;
        self.force_intra = prediction == 0;
    }

    /// `CELT_SET_ANALYSIS`: what the tonality analysis says of the next frames.
    pub(crate) fn set_analysis(&mut self, info: &AnalysisInfo) {
        self.analysis = *info;
    }

    /// `OPUS_SET_ENERGY_MASK`: how far each band of each channel stands above what the
    /// other channels of a surround mix mask (log2 units).
    pub(crate) fn set_energy_mask(&mut self, mask: Option<&[f32; 2 * NB_EBANDS]>) {
        self.energy_mask = mask.copied();
    }

    /// `OPUS_SET_LFE`.
    pub(crate) fn set_lfe(&mut self, lfe: bool) {
        self.lfe = lfe;
    }

    /// `OPUS_GET_FINAL_RANGE`: the range coder's state after the last frame.
    pub(crate) fn final_range(&self) -> u32 {
        self.rng
    }

    /// `run_prefilter`: chooses the pitch pre-filter (when `enabled`) and applies it into
    /// `input` (`n + OVERLAP` per channel: the overlap from the last frame, then the frame).
    /// Returns whether it is on, its period, gain and quantised gain.
    fn run_prefilter(
        &mut self,
        input: &mut [f32],
        n: usize,
        tapset: usize,
        enabled: bool,
        nb_available_bytes: i32,
    ) -> (bool, i32, f32, u32) {
        let cc = self.channels;
        let len = n + OVERLAP;
        let mut pre = [[0.0f32; 960 + COMBFILTER_MAXPERIOD]; 2];
        for ch in 0..cc {
            pre[ch][..COMBFILTER_MAXPERIOD].copy_from_slice(&self.prefilter_mem[ch]);
            pre[ch][COMBFILTER_MAXPERIOD..COMBFILTER_MAXPERIOD + n]
                .copy_from_slice(&input[ch * len + OVERLAP..ch * len + OVERLAP + n]);
        }
        let (mut gain1, pitch_index) = if enabled {
            self.find_pitch(&pre, n)
        } else {
            (0.0, COMBFILTER_MINPERIOD)
        };
        if self.analysis.valid {
            gain1 *= self.analysis.max_pitch_ratio;
        }
        // The gain threshold, from the rate and the continuity.
        let mut pf_threshold = 0.2f32;
        if (pitch_index - self.prefilter_period).abs() * 10 > pitch_index {
            pf_threshold += 0.2;
        }
        if nb_available_bytes < 25 {
            pf_threshold += 0.1;
        }
        if nb_available_bytes < 35 {
            pf_threshold += 0.1;
        }
        if self.prefilter_gain > 0.4 {
            pf_threshold -= 0.1;
        }
        if self.prefilter_gain > 0.55 {
            pf_threshold -= 0.1;
        }
        pf_threshold = pf_threshold.max(0.2);
        let (pf_on, qg) = if gain1 < pf_threshold {
            gain1 = 0.0;
            (false, 0)
        } else {
            if (gain1 - self.prefilter_gain).abs() < 0.1 {
                gain1 = self.prefilter_gain;
            }
            let qg = (libm::floorf(0.5 + gain1 * 32.0 / 3.0) as i32 - 1).clamp(0, 7);
            gain1 = 0.093_75 * (qg + 1) as f32;
            (true, qg as u32)
        };
        self.prefilter_period = self.prefilter_period.max(COMBFILTER_MINPERIOD);
        for ch in 0..cc {
            let base = ch * len;
            input[base..base + OVERLAP].copy_from_slice(&self.in_mem[ch]);
            // shortMdctSize == overlap: no steady part before the cross-fade.
            comb_filter(
                &mut input[base + OVERLAP..base + OVERLAP + n],
                &pre[ch],
                COMBFILTER_MAXPERIOD,
                self.prefilter_period,
                pitch_index,
                n,
                -self.prefilter_gain,
                -gain1,
                self.prefilter_tapset,
                tapset,
            );
            self.in_mem[ch].copy_from_slice(&input[base + n..base + n + OVERLAP]);
            let mem = &mut self.prefilter_mem[ch];
            if n > COMBFILTER_MAXPERIOD {
                mem.copy_from_slice(&pre[ch][n..n + COMBFILTER_MAXPERIOD]);
            } else {
                mem.copy_within(n.., 0);
                mem[COMBFILTER_MAXPERIOD - n..]
                    .copy_from_slice(&pre[ch][COMBFILTER_MAXPERIOD..COMBFILTER_MAXPERIOD + n]);
            }
        }
        (pf_on, pitch_index, gain1, qg)
    }

    /// The pitch search of `run_prefilter`: its gain and period.
    fn find_pitch(&self, pre: &[[f32; 960 + COMBFILTER_MAXPERIOD]; 2], n: usize) -> (f32, i32) {
        let len = COMBFILTER_MAXPERIOD + n;
        let mut pitch_buf = [0.0f32; (960 + COMBFILTER_MAXPERIOD) >> 1];
        let pitch_buf = &mut pitch_buf[..len >> 1];
        let cc = self.channels;
        pitch_downsample([&pre[0][..len], &pre[cc - 1][..len]], cc, pitch_buf);
        // Not the first 1.5 octave of the range: too many false positives from short-term
        // correlation.
        let max_pitch = COMBFILTER_MAXPERIOD - 3 * COMBFILTER_MINPERIOD as usize;
        let found = pitch_search(
            &pitch_buf[COMBFILTER_MAXPERIOD >> 1..],
            pitch_buf,
            n,
            max_pitch,
        );
        let mut pitch_index = COMBFILTER_MAXPERIOD - found;
        let gain = remove_doubling(
            pitch_buf,
            COMBFILTER_MAXPERIOD,
            COMBFILTER_MINPERIOD as usize,
            n,
            &mut pitch_index,
            self.prefilter_period.max(0) as usize,
            self.prefilter_gain,
        );
        let pitch_index = pitch_index.min(COMBFILTER_MAXPERIOD - 2);
        (0.7 * gain, pitch_index as i32)
    }

    /// Encodes one frame of `frame.frame_size * channels` interleaved samples (full scale 1.0)
    /// into `enc`, per the module's contract; returns the frame's size in bytes.
    pub(crate) fn encode(
        &mut self,
        pcm: &[f32],
        frame: &CeltEncodeFrame,
        enc: &mut RangeEncoder,
    ) -> Result<usize> {
        let cc = self.channels;
        let c = frame.stream_channels.clamp(1, cc);
        let start = frame.start_band;
        let end = frame.end_band;
        let hybrid = start != 0;
        let lm = match frame.frame_size {
            120 => 0,
            240 => 1,
            480 => 2,
            960 => 3,
            _ => return Err(Error::BadArgument("a CELT frame is 2.5, 5, 10 or 20 ms")),
        };
        if start >= end || end > NB_EBANDS || pcm.len() != frame.frame_size * cc {
            return Err(Error::BadArgument("CELT bands or input out of range"));
        }
        let mut nb_compressed_bytes = enc.storage() as i32;
        if nb_compressed_bytes < 2 {
            return Err(Error::BadArgument("a CELT frame has at least 2 bytes"));
        }
        let m = 1usize << lm;
        let n = m * SHORT_MDCT_SIZE;
        let ci = c as i32;
        let mut tf_estimate = 0.0f32;

        let tell0_frac = enc.tell_frac() as i32;
        let mut tell = enc.tell();
        let nb_filled_bytes = (tell + 4) >> 3;
        let _ = tell0_frac;
        nb_compressed_bytes = nb_compressed_bytes.min(1275);
        let mut nb_available_bytes = nb_compressed_bytes - nb_filled_bytes;
        let bitrate = frame.bitrate.map(|b| b.min(260_000 * cc as i32));
        let vbr_rate;
        let mut effective_bytes;
        match bitrate {
            Some(b) if frame.vbr => {
                let den = 48_000 >> BITRES;
                vbr_rate = (b * frame.frame_size as i32 + (den >> 1)) / den;
                effective_bytes = vbr_rate >> (3 + BITRES);
            }
            _ => {
                vbr_rate = 0;
                if let Some(b) = bitrate {
                    let mut tmp = b * frame.frame_size as i32;
                    if tell > 1 {
                        tmp += tell * 48_000;
                    }
                    nb_compressed_bytes = nb_compressed_bytes
                        .min((tmp + 4 * 48_000) / (8 * 48_000))
                        .max(2);
                    enc.shrink(nb_compressed_bytes as usize);
                }
                effective_bytes = nb_compressed_bytes - nb_filled_bytes;
            }
        }
        let overhead = (40 * ci + 20) * ((400 >> lm) - 50);
        let mut equiv_rate = ((nb_compressed_bytes * 8 * 50) << (3 - lm)) - overhead;
        if let Some(b) = bitrate {
            equiv_rate = equiv_rate.min(b - overhead);
        }
        if vbr_rate > 0 && frame.constrained_vbr {
            // The most VBR may use, up front, so that the bust-prevention logic below
            // triggers properly. At least two bytes when nothing else is in the frame.
            let vbr_bound = vbr_rate;
            let max_allowed = (if tell == 1 { 2 } else { 0 })
                .max((vbr_rate + vbr_bound - self.vbr_reservoir) >> (BITRES + 3))
                .min(nb_available_bytes);
            if max_allowed < nb_available_bytes {
                nb_compressed_bytes = nb_filled_bytes + max_allowed;
                nb_available_bytes = max_allowed;
                enc.shrink(nb_compressed_bytes as usize);
            }
        }
        let mut total_bits = nb_compressed_bytes * 8;
        let eff_end = end.min(NB_EBANDS);

        let len = n + OVERLAP;
        let mut input = vec![0.0f32; cc * len];
        let head = c * (n - OVERLAP);
        let mut sample_max = self.overlap_max.max(maxabs(&pcm[..head]));
        self.overlap_max = maxabs(&pcm[head..head + c * OVERLAP]);
        sample_max = sample_max.max(self.overlap_max);
        let mut silence = sample_max <= 1.0 / (1 << LSB_DEPTH) as f32;
        if tell == 1 {
            enc.enc_bit_logp(silence, 15);
        } else {
            silence = false;
        }
        if silence {
            // VBR needs no more than the minimum.
            if vbr_rate > 0 {
                nb_compressed_bytes = nb_compressed_bytes.min(nb_filled_bytes + 2);
                effective_bytes = nb_compressed_bytes;
                total_bits = nb_compressed_bytes * 8;
                nb_available_bytes = 2;
                enc.shrink(nb_compressed_bytes as usize);
            }
            // The rest of the frame is zeros, which the buffer already holds.
            tell = nb_compressed_bytes * 8;
            enc.mark_written_to(tell);
        }
        let need_clip = sample_max > 65536.0;
        for ch in 0..cc {
            let out = &mut input[ch * len + OVERLAP..ch * len + len];
            let mut mem = self.preemph_mem[ch];
            for (i, o) in out.iter_mut().enumerate() {
                let mut x = pcm[cc * i + ch] * 32768.0;
                if need_clip {
                    // Non-portable files otherwise.
                    x = x.clamp(-65536.0, 65536.0);
                }
                *o = x - mem;
                mem = PREEMPH * x;
            }
            self.preemph_mem[ch] = mem;
        }

        let enabled = ((self.lfe && nb_available_bytes > 3) || nb_available_bytes > 12 * ci)
            && !hybrid
            && !silence
            && !self.disable_pf
            && COMPLEXITY >= 5;
        let prefilter_tapset = self.tapset_decision;
        let (pf_on, pitch_index, gain1, qg) =
            self.run_prefilter(&mut input, n, prefilter_tapset, enabled, nb_available_bytes);
        let period = f64::from(self.prefilter_period);
        let pitch_change = (gain1 > 0.4 || self.prefilter_gain > 0.4)
            && (!self.analysis.valid || f64::from(self.analysis.tonality) > 0.3)
            && (f64::from(pitch_index) > 1.26 * period || f64::from(pitch_index) < 0.79 * period);
        if !pf_on {
            if !hybrid && tell + 16 <= total_bits {
                enc.enc_bit_logp(false, 1);
            }
        } else {
            // Not gated on the bits left: nb_available_bytes was checked.
            enc.enc_bit_logp(true, 1);
            let pi = pitch_index + 1;
            let octave = (32 - (pi as u32).leading_zeros()) as i32 - 5;
            enc.enc_uint(octave as u32, 6);
            enc.enc_bits((pi - (16 << octave)) as u32, (4 + octave) as u32);
            enc.enc_bits(qg, 3);
            enc.enc_icdf(prefilter_tapset, &TAPSET_ICDF, 2);
        }

        let mut is_transient = false;
        let mut short_blocks = false;
        let mut tf_chan = 0;
        let mut weak_transient = false;
        if COMPLEXITY >= 1 && !self.lfe {
            // Steadier energy on fricatives at low hybrid rates; vowels (voiced, a small
            // SILK offset) keep their real transients.
            let allow_weak = hybrid && effective_bytes < 15 && frame.silk_info.signal_type != 2;
            let (t, est, chan, weak) = transient_analysis(&input, len, cc, allow_weak);
            is_transient = t;
            tf_estimate = est;
            tf_chan = chan;
            weak_transient = weak;
        }
        let mut transient_got_disabled = false;
        if lm > 0 && enc.tell() + 3 <= total_bits {
            short_blocks = is_transient;
        } else {
            is_transient = false;
            transient_got_disabled = true;
        }

        let mut freq = vec![0.0f32; cc * n];
        let mut band_e = [0.0f32; 2 * NB_EBANDS];
        let mut band_log_e = [0.0f32; 2 * NB_EBANDS];
        let mut band_log_e2 = [0.0f32; 2 * NB_EBANDS];
        let second_mdct = short_blocks && COMPLEXITY >= 8;
        if second_mdct {
            compute_mdcts(&input, &mut freq, false, c, cc, lm);
            compute_band_energies(&freq, &mut band_e, eff_end, c, lm);
            amp2_log2(&band_e, &mut band_log_e2, eff_end, end, c);
            for ch in 0..c {
                for i in 0..end {
                    band_log_e2[NB_EBANDS * ch + i] += 0.5 * lm as f32;
                }
            }
        }
        compute_mdcts(&input, &mut freq, short_blocks, c, cc, lm);
        if cc == 2 && c == 1 {
            tf_chan = 0;
        }
        compute_band_energies(&freq, &mut band_e, eff_end, c, lm);
        if self.lfe {
            // Past the second band, at most 40 dB under the first.
            for i in 2..end {
                band_e[i] = band_e[i].min(1e-4 * band_e[0]).max(EPSILON);
            }
        }
        amp2_log2(&band_e, &mut band_log_e, eff_end, end, c);

        // How much the other channels of a surround mix mask this stream's.
        let mut surround_dynalloc = [0.0f32; NB_EBANDS];
        let mut surround_masking = 0.0f32;
        let mut surround_trim = 0.0f32;
        if let (false, Some(mask), false) = (hybrid, self.energy_mask.as_ref(), self.lfe) {
            let mask_end = self.last_coded_bands.max(2);
            let (mut mask_avg, mut diff, mut count) = (0.0f32, 0.0f32, 0i32);
            for ch in 0..c {
                for i in 0..mask_end {
                    let mut m = mask[NB_EBANDS * ch + i].clamp(-2.0, 0.25);
                    if m > 0.0 {
                        m *= 0.5;
                    }
                    let width = (eb(i + 1) - eb(i)) as i32;
                    mask_avg += m * width as f32;
                    count += width;
                    diff += m * (1 + 2 * i as i32 - mask_end as i32) as f32;
                }
            }
            mask_avg /= count as f32;
            mask_avg += 0.2;
            let me = mask_end as i32;
            diff = diff * 6.0 / (ci * (me - 1) * (me + 1) * me) as f32;
            // Again, conservatively.
            diff *= 0.5;
            diff = diff.clamp(-0.031, 0.031);
            // The band in the middle of the coded spectrum.
            let mut midband = 0;
            while eb(midband + 1) < eb(mask_end) / 2 {
                midband += 1;
            }
            let mut count_dynalloc = 0;
            for i in 0..mask_end {
                let lin = mask_avg + diff * (i as i32 - midband as i32) as f32;
                let unmask = if c == 2 {
                    mask[i].max(mask[NB_EBANDS + i])
                } else {
                    mask[i]
                };
                let unmask = unmask.min(0.0) - lin;
                if unmask > 0.25 {
                    surround_dynalloc[i] = unmask - 0.25;
                    count_dynalloc += 1;
                }
            }
            if count_dynalloc >= 3 {
                // Dynalloc in many bands: the masking rate was probably too low.
                mask_avg += 0.25;
                if mask_avg > 0.0 {
                    // Something went really wrong: no masking.
                    mask_avg = 0.0;
                    diff = 0.0;
                    surround_dynalloc[..mask_end].fill(0.0);
                } else {
                    for v in &mut surround_dynalloc[..mask_end] {
                        *v = (*v - 0.25).max(0.0);
                    }
                }
            }
            mask_avg += 0.2;
            // In the trim's 1/64 steps.
            surround_trim = 64.0 * diff;
            surround_masking = mask_avg;
        }

        // Temporal VBR, not for the LFE.
        let mut temporal_vbr = 0.0;
        if !self.lfe {
            let mut follow = -10.0f32;
            let mut frame_avg = 0.0f32;
            let offset = if short_blocks { 0.5 * lm as f32 } else { 0.0 };
            for i in start..end {
                follow = (follow - 1.0).max(band_log_e[i] - offset);
                if c == 2 {
                    follow = follow.max(band_log_e[i + NB_EBANDS] - offset);
                }
                frame_avg += follow;
            }
            frame_avg /= (end - start) as f32;
            temporal_vbr = (frame_avg - self.spec_avg).clamp(-1.5, 3.0f32);
            self.spec_avg += 0.02 * temporal_vbr;
        }
        if !second_mdct {
            band_log_e2 = band_log_e;
        }

        // A last chance for a transient the time analysis missed.
        if lm > 0
            && enc.tell() + 3 <= total_bits
            && !is_transient
            && COMPLEXITY >= 5
            && !self.lfe
            && !hybrid
            && patch_transient_decision(&band_log_e, &self.old_band_e, start, end, c)
        {
            is_transient = true;
            short_blocks = true;
            compute_mdcts(&input, &mut freq, true, c, cc, lm);
            compute_band_energies(&freq, &mut band_e, eff_end, c, lm);
            amp2_log2(&band_e, &mut band_log_e, eff_end, end, c);
            // Short MDCTs scale differently from long ones.
            for ch in 0..c {
                for i in 0..end {
                    band_log_e2[NB_EBANDS * ch + i] += 0.5 * lm as f32;
                }
            }
            tf_estimate = 0.2;
        }
        if lm > 0 && enc.tell() + 3 <= total_bits {
            enc.enc_bit_logp(is_transient, 3);
        }

        let mut x = vec![0.0f32; c * n];
        normalise_bands(&freq, &mut x, &band_e, eff_end, c, m);

        let enable_tf_analysis =
            effective_bytes >= 15 * ci && !hybrid && COMPLEXITY >= 2 && !self.lfe;
        let dyn_ = dynalloc_analysis(
            &band_log_e,
            &band_log_e2,
            &self.old_band_e,
            start,
            end,
            c,
            is_transient,
            frame.vbr,
            frame.constrained_vbr,
            lm,
            effective_bytes,
            self.lfe,
            &surround_dynalloc,
            &self.analysis,
        );
        let mut offsets = dyn_.offsets;
        let mut tf_res = [0i32; NB_EBANDS];
        let tf_select;
        if enable_tf_analysis {
            let lambda = 80.max(20480 / effective_bytes + 2);
            tf_select = tf_analysis(
                eff_end,
                is_transient,
                &mut tf_res,
                lambda,
                &x,
                n,
                lm,
                tf_estimate,
                tf_chan,
                &dyn_.importance,
            );
            for i in eff_end..end {
                tf_res[i] = tf_res[eff_end - 1];
            }
        } else if hybrid && weak_transient {
            // TF on a long window improves time resolution imperfectly, which at low rates
            // keeps the energy from collapsing.
            tf_res[..end].fill(1);
            tf_select = 0;
        } else if hybrid && effective_bytes < 15 && frame.silk_info.signal_type != 2 {
            // Low-rate hybrid: 5 ms time resolution rather than 2.5 ms.
            tf_res[..end].fill(0);
            tf_select = usize::from(is_transient);
        } else {
            for r in tf_res.iter_mut().take(end) {
                *r = i32::from(is_transient);
            }
            tf_select = 0;
        }

        let mut error = [0.0f32; 2 * NB_EBANDS];
        for ch in 0..c {
            for i in start..end {
                let k = i + ch * NB_EBANDS;
                // Stable energy: lean towards the last error, for a steadier gain.
                if (band_log_e[k] - self.old_band_e[k]).abs() < 2.0 {
                    band_log_e[k] -= self.energy_error[k] * 0.25;
                }
            }
        }
        if c == 1 && cc == 2 {
            // The decoder predicts a mono frame from the louder channel: so must we.
            for i in 0..NB_EBANDS {
                self.old_band_e[i] = self.old_band_e[i].max(self.old_band_e[NB_EBANDS + i]);
            }
        }
        quant_coarse_energy(
            &Coarse {
                start,
                end,
                eff_end,
                budget: total_bits,
                c,
                lm,
                nb_available_bytes,
                force_intra: self.force_intra,
                two_pass: COMPLEXITY >= 4,
                lfe: self.lfe,
            },
            &band_log_e,
            &mut self.old_band_e,
            &mut error,
            enc,
            &mut self.delayed_intra,
        );

        tf_encode(start, end, is_transient, &mut tf_res, lm, tf_select, enc);

        if enc.tell() + 4 <= total_bits {
            if self.lfe {
                self.tapset_decision = 0;
                self.spread_decision = SPREAD_NORMAL;
            } else if hybrid {
                self.spread_decision = if is_transient {
                    SPREAD_NORMAL
                } else {
                    SPREAD_AGGRESSIVE
                };
            } else if short_blocks || COMPLEXITY < 3 || nb_available_bytes < 10 * ci {
                self.spread_decision = SPREAD_NORMAL;
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
                    m,
                    &dyn_.spread_weight,
                );
            }
            enc.enc_icdf(self.spread_decision as usize, &SPREAD_ICDF, 5);
        }

        // For the LFE, everything interesting is in the first band.
        if self.lfe {
            offsets[0] = 8.min(effective_bytes / 3);
        }
        let cap = init_caps(lm, c);
        let mut dynalloc_logp = 6;
        total_bits <<= BITRES;
        let mut total_boost = 0;
        let mut tell_frac = enc.tell_frac() as i32;
        for i in start..end {
            let width = ((c * (eb(i + 1) - eb(i))) << lm) as i32;
            // 6 bits, but no more than one per sample and no less than 1/8.
            let quanta = (width << BITRES).min((6 << BITRES).max(width));
            let mut loop_logp = dynalloc_logp;
            let mut boost = 0;
            let mut j = 0;
            while tell_frac + (loop_logp << BITRES) < total_bits - total_boost && boost < cap[i] {
                let flag = j < offsets[i];
                enc.enc_bit_logp(flag, loop_logp as u32);
                tell_frac = enc.tell_frac() as i32;
                if !flag {
                    break;
                }
                boost += quanta;
                total_boost += quanta;
                loop_logp = 1;
                j += 1;
            }
            // Dynalloc gets likelier.
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
            // Always mid/side at 2.5 ms, until a better analysis.
            if lm != 0 {
                dual_stereo = stereo_analysis(&x, lm, n);
            }
            self.intensity = hysteresis_decision(
                (equiv_rate / 1000) as f32,
                &INTENSITY_THRESHOLDS,
                &INTENSITY_HYSTERESIS,
                self.intensity,
            )
            .clamp(start, end);
        }

        let mut alloc_trim = 5;
        if tell_frac + (6 << BITRES) <= total_bits - total_boost {
            if hybrid || self.lfe {
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
                    surround_trim,
                    &self.analysis,
                );
            }
            enc.enc_icdf(alloc_trim as usize, &TRIM_ICDF, 7);
            tell_frac = enc.tell_frac() as i32;
        }

        if vbr_rate > 0 {
            let lm_diff = MAX_LM as i32 - lm as i32;
            // No more than 510 kb/s, whatever the frame size: the allocator could not use it.
            nb_compressed_bytes = nb_compressed_bytes.min(1275 >> (3 - lm));
            let mut base_target = if hybrid {
                (vbr_rate - ((9 * ci + 4) << BITRES)).max(0)
            } else {
                vbr_rate - ((40 * ci + 20) << BITRES)
            };
            if frame.constrained_vbr {
                base_target += self.vbr_offset >> lm_diff;
            }
            let mut target = if hybrid {
                let mut t = base_target;
                // Tonal frames (a SILK offset under 100) need more bits than noisy ones.
                if frame.silk_info.offset < 100 {
                    t += 12 << BITRES >> (3 - lm);
                }
                if frame.silk_info.offset > 100 {
                    t -= 18 << BITRES >> (3 - lm);
                }
                t += ((tf_estimate - 0.25) * (50 << BITRES) as f32) as i32;
                // A strong transient gets enough to fold rather than fill the first bands.
                if tf_estimate > 0.7 {
                    t = t.max(50 << BITRES);
                }
                t
            } else {
                compute_vbr(
                    &self.analysis,
                    base_target,
                    lm,
                    equiv_rate,
                    self.last_coded_bands,
                    c,
                    self.intensity,
                    frame.constrained_vbr,
                    self.stereo_saving,
                    dyn_.tot_boost,
                    tf_estimate,
                    dyn_.max_depth,
                    temporal_vbr,
                    pitch_change,
                    self.lfe,
                    self.energy_mask.map(|_| surround_masking),
                )
            };
            // Less the offset, plus what was spent so far.
            target += tell_frac;
            // Never so small that the encoder would run out of bits: the margin of two bytes
            // keeps the decoder's bust prevention from having triggered so far.
            let mut min_allowed =
                ((tell_frac + total_boost + (1 << (BITRES + 3)) - 1) >> (BITRES + 3)) + 2;
            if hybrid {
                // The 37 bits of a redundant frame's signalling.
                min_allowed = min_allowed.max(
                    (tell0_frac + (37 << BITRES) + total_boost + (1 << (BITRES + 3)) - 1)
                        >> (BITRES + 3),
                );
            }
            nb_available_bytes = ((target + (1 << (BITRES + 2))) >> (BITRES + 3))
                .max(min_allowed)
                .min(nb_compressed_bytes);
            // How far the frame missed the target.
            let mut delta = target - vbr_rate;
            target = nb_available_bytes << (BITRES + 3);
            // Silence does not move the drift (the encoder would then overshoot after it),
            // but lets the reservoir refill.
            if silence {
                nb_available_bytes = 2;
                target = (2 * 8) << BITRES;
                delta = 0;
            }
            let alpha = if self.vbr_count < 970 {
                self.vbr_count += 1;
                1.0 / (self.vbr_count + 20) as f32
            } else {
                0.001
            };
            if frame.constrained_vbr {
                self.vbr_reservoir += target - vbr_rate;
                self.vbr_drift += (alpha
                    * ((delta * (1 << lm_diff)) - self.vbr_offset - self.vbr_drift) as f32)
                    as i32;
                self.vbr_offset = -self.vbr_drift;
                if self.vbr_reservoir < 0 {
                    // Under the minimum: more bits, unless only silence is coded.
                    let adjust = (-self.vbr_reservoir) / (8 << BITRES);
                    if !silence {
                        nb_available_bytes += adjust;
                    }
                    self.vbr_reservoir = 0;
                }
            }
            nb_compressed_bytes = nb_compressed_bytes.min(nb_available_bytes);
            enc.shrink(nb_compressed_bytes as usize);
        }

        // The frame, less what is written, less a safety bit.
        let mut bits = ((nb_compressed_bytes * 8) << BITRES) - enc.tell_frac() as i32 - 1;
        let anti_collapse_rsv = if is_transient && lm >= 2 && bits >= (lm as i32 + 2) << BITRES {
            1 << BITRES
        } else {
            0
        };
        bits -= anti_collapse_rsv;
        let mut signal_bandwidth = end - 1;
        if self.analysis.valid {
            let ci = c as i32;
            let min_bandwidth = if equiv_rate < 32_000 * ci {
                13
            } else if equiv_rate < 48_000 * ci {
                16
            } else if equiv_rate < 60_000 * ci {
                18
            } else if equiv_rate < 80_000 * ci {
                19
            } else {
                20
            };
            signal_bandwidth = (self.analysis.bandwidth.max(0) as usize).max(min_bandwidth);
        }
        if self.lfe {
            signal_bandwidth = 1;
        }
        let alloc = compute_allocation(
            start,
            end,
            &offsets,
            &cap,
            alloc_trim,
            bits,
            c,
            lm,
            &mut AllocEncoder {
                enc,
                intensity: self.intensity,
                dual_stereo,
                prev: self.last_coded_bands,
                signal_bandwidth,
            },
        );
        self.intensity = alloc.intensity;
        self.last_coded_bands = if self.last_coded_bands != 0 {
            (self.last_coded_bands + 1).min((self.last_coded_bands - 1).max(alloc.coded_bands))
        } else {
            alloc.coded_bands
        };

        quant_fine_energy(
            &mut self.old_band_e,
            &mut error,
            start,
            end,
            &alloc.fine_quant,
            enc,
            c,
        );

        let mut collapse_masks = [0u8; 2 * NB_EBANDS];
        {
            let (xa, ya) = x.split_at_mut(n);
            let bands = EncodeBands {
                start,
                end,
                band_e: &band_e,
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
                theta_rdo: COMPLEXITY >= 8,
            };
            let y = (c == 2).then_some(ya);
            quant_all_bands_enc(&bands, xa, y, &mut collapse_masks, enc, &mut self.rng);
        }

        if anti_collapse_rsv > 0 {
            let anti_collapse_on = self.consec_transient < 2;
            enc.enc_bits(u32::from(anti_collapse_on), 1);
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
            enc,
            c,
        );
        self.energy_error = [0.0; 2 * NB_EBANDS];
        for ch in 0..c {
            for i in start..end {
                let k = i + ch * NB_EBANDS;
                self.energy_error[k] = error[k].clamp(-0.5, 0.5);
            }
        }
        if silence {
            for v in &mut self.old_band_e[..c * NB_EBANDS] {
                *v = -28.0;
            }
        }

        self.prefilter_period = pitch_index;
        self.prefilter_gain = gain1;
        self.prefilter_tapset = prefilter_tapset;

        if cc == 2 && c == 1 {
            self.old_band_e.copy_within(0..NB_EBANDS, NB_EBANDS);
        }
        if !is_transient {
            self.old_log_e2 = self.old_log_e;
            self.old_log_e = self.old_band_e;
        } else {
            for (l, &e) in self.old_log_e.iter_mut().zip(&self.old_band_e) {
                *l = l.min(e);
            }
        }
        // In case start or end change.
        for ch in 0..cc {
            for i in (0..start).chain(end..NB_EBANDS) {
                let k = ch * NB_EBANDS + i;
                self.old_band_e[k] = 0.0;
                self.old_log_e[k] = -28.0;
                self.old_log_e2[k] = -28.0;
            }
        }
        if is_transient || transient_got_disabled {
            self.consec_transient += 1;
        } else {
            self.consec_transient = 0;
        }
        self.rng = enc.rng();
        if enc.error() {
            return Err(Error::Corrupt("the CELT encoder overflowed its frame"));
        }
        Ok(nb_compressed_bytes as usize)
    }
}

#[cfg(test)]
#[path = "encoder_tests.rs"]
mod tests;
