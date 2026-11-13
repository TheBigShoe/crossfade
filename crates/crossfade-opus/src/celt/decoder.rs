//! `celt_decode_with_ec` and `celt_decode_lost` (`celt/celt.c`, the decoder half): the frame's
//! symbols in the reference's order, synthesis through the IMDCT, the comb post-filter and
//! de-emphasis, and the concealment of lost frames.
#![allow(clippy::needless_range_loop)] // index loops as the reference writes them

use super::CeltFrame;
use super::bands::{AllBands, anti_collapse, denormalise_bands, quant_all_bands};
use super::ec::Ec;
use super::lpc::{LPC_ORDER, autocorr, fir, iir, lpc, pitch_downsample, pitch_search};
use super::maths::lcg_rand;
use super::mdct;
use super::quant_bands::{
    log2_amp, unquant_coarse_energy, unquant_energy_finalise, unquant_fine_energy,
};
use super::rate::{BITRES, compute_allocation, init_caps};
use super::tables::{
    COMB_GAINS, EBANDS, MAX_LM, NB_EBANDS, OVERLAP, PREEMPH, SHORT_MDCT_SIZE, SPREAD_ICDF,
    TAPSET_ICDF, TF_SELECT_TABLE, TRIM_ICDF, WINDOW,
};
use super::vq::renormalise_vector;
use crate::error::{Error, Result};
use crate::range::RangeDecoder;

/// Samples of history per channel (`DECODE_BUFFER_SIZE`).
const DECODE_BUFFER_SIZE: usize = 2048;
/// The longest pitch period (`MAX_PERIOD`).
const MAX_PERIOD: usize = 1024;
const COMBFILTER_MINPERIOD: i32 = 15;
const SPREAD_NORMAL: i32 = 2;
/// The largest frame CELT reads (`len > 1275` is a bad argument).
const MAX_FRAME_BYTES: usize = 1275;

/// The CELT decoder's state for one stream (libopus `CELTDecoder`).
#[derive(Debug, Clone)]
pub(crate) struct CeltDecoder {
    /// Output channels, 1 or 2 (`CC`).
    channels: usize,
    /// Bands and coded channels of the last frame set (`st->start`, `end`, `stream_channels`).
    start: usize,
    end: usize,
    stream_channels: usize,
    rng: u32,
    last_pitch_index: usize,
    loss_count: u32,
    postfilter_period: i32,
    postfilter_period_old: i32,
    postfilter_gain: f32,
    postfilter_gain_old: f32,
    postfilter_tapset: usize,
    postfilter_tapset_old: usize,
    preemph_mem: [f32; 2],
    /// Per channel: `DECODE_BUFFER_SIZE` samples of output history, then the `OVERLAP`
    /// samples of the next frame's overlap (`_decode_mem`).
    decode_mem: [Vec<f32>; 2],
    lpc: [[f32; LPC_ORDER]; 2],
    old_band_e: [f32; 2 * NB_EBANDS],
    old_log_e: [f32; 2 * NB_EBANDS],
    old_log_e2: [f32; 2 * NB_EBANDS],
    background_log_e: [f32; 2 * NB_EBANDS],
}

/// `comb_filter` in place on `buf[at..at + n]`, reading up to `t + 2` samples before `at`:
/// the post-filter cross-faded from (`t0`, `g0`, `tapset0`) to (`t1`, `g1`, `tapset1`) over
/// `overlap` samples with the squared window.
#[allow(clippy::too_many_arguments)]
fn comb_filter(
    buf: &mut [f32],
    at: usize,
    t0: i32,
    t1: i32,
    n: usize,
    g0: f32,
    g1: f32,
    tapset0: usize,
    tapset1: usize,
    overlap: usize,
) {
    // No filter on either side: nothing to add (libopus's early return).
    if g0 == 0.0 && g1 == 0.0 {
        return;
    }
    let t0 = t0 as usize;
    let t1 = t1 as usize;
    let g00 = g0 * COMB_GAINS[tapset0][0];
    let g01 = g0 * COMB_GAINS[tapset0][1];
    let g02 = g0 * COMB_GAINS[tapset0][2];
    let g10 = g1 * COMB_GAINS[tapset1][0];
    let g11 = g1 * COMB_GAINS[tapset1][1];
    let g12 = g1 * COMB_GAINS[tapset1][2];
    let overlap = overlap.min(n);
    for i in 0..overlap {
        let x = at + i;
        let f = WINDOW[i] * WINDOW[i];
        let nf = 1.0 - f;
        buf[x] = buf[x]
            + ((nf * g00) * buf[x - t0])
            + ((nf * g01) * buf[x - t0 - 1])
            + ((nf * g01) * buf[x - t0 + 1])
            + ((nf * g02) * buf[x - t0 - 2])
            + ((nf * g02) * buf[x - t0 + 2])
            + ((f * g10) * buf[x - t1])
            + ((f * g11) * buf[x - t1 - 1])
            + ((f * g11) * buf[x - t1 + 1])
            + ((f * g12) * buf[x - t1 - 2])
            + ((f * g12) * buf[x - t1 + 2]);
    }
    comb_filter_const(buf, at + overlap, t1, n - overlap, g10, g11, g12);
}

/// The steady part of [`comb_filter`]: taps `g10`, `g11`, `g12` at lag `t`.
fn comb_filter_const(buf: &mut [f32], at: usize, t: usize, n: usize, g10: f32, g11: f32, g12: f32) {
    if g10 == 0.0 && g11 == 0.0 && g12 == 0.0 {
        return;
    }
    for x in at..at + n {
        buf[x] = buf[x]
            + g10 * buf[x - t]
            + g11 * buf[x - t - 1]
            + g11 * buf[x - t + 1]
            + g12 * buf[x - t - 2]
            + g12 * buf[x - t + 2];
    }
}

/// `tf_decode`.
fn tf_decode(
    start: usize,
    end: usize,
    transient: bool,
    tf_res: &mut [i32; NB_EBANDS],
    lm: usize,
    ec: &mut Ec<'_, '_>,
) {
    let mut budget = (ec.storage() * 8) as u32;
    let mut tell = ec.tell() as u32;
    let mut logp: u32 = if transient { 2 } else { 4 };
    let tf_select_rsv = lm > 0 && tell + logp < budget;
    budget -= u32::from(tf_select_rsv);
    let mut curr = 0;
    let mut tf_changed = 0;
    for r in tf_res.iter_mut().take(end).skip(start) {
        if tell + logp <= budget {
            curr ^= i32::from(ec.bit_logp(logp));
            tell = ec.tell() as u32;
            tf_changed |= curr;
        }
        *r = curr;
        logp = if transient { 4 } else { 5 };
    }
    let t = 4 * usize::from(transient);
    let mut tf_select = 0;
    if tf_select_rsv
        && TF_SELECT_TABLE[lm][t + tf_changed as usize]
            != TF_SELECT_TABLE[lm][t + 2 + tf_changed as usize]
    {
        tf_select = usize::from(ec.bit_logp(1));
    }
    for r in tf_res.iter_mut().take(end).skip(start) {
        *r = i32::from(TF_SELECT_TABLE[lm][t + 2 * tf_select + *r as usize]);
    }
}

impl CeltDecoder {
    /// A decoder writing `channels` (1 or 2) output channels (`celt_decoder_init` at 48 kHz).
    pub(crate) fn new(channels: usize) -> Self {
        let channels = channels.clamp(1, 2);
        let mut d = Self {
            channels,
            start: 0,
            end: NB_EBANDS,
            stream_channels: channels,
            rng: 0,
            last_pitch_index: 0,
            loss_count: 0,
            postfilter_period: 0,
            postfilter_period_old: 0,
            postfilter_gain: 0.0,
            postfilter_gain_old: 0.0,
            postfilter_tapset: 0,
            postfilter_tapset_old: 0,
            preemph_mem: [0.0; 2],
            decode_mem: [
                vec![0.0; DECODE_BUFFER_SIZE + OVERLAP],
                vec![0.0; DECODE_BUFFER_SIZE + OVERLAP],
            ],
            lpc: [[0.0; LPC_ORDER]; 2],
            old_band_e: [0.0; 2 * NB_EBANDS],
            old_log_e: [0.0; 2 * NB_EBANDS],
            old_log_e2: [0.0; 2 * NB_EBANDS],
            background_log_e: [0.0; 2 * NB_EBANDS],
        };
        d.reset();
        d
    }

    /// Forgets the stream, as `OPUS_RESET_STATE` does; the bands and coded channels last set
    /// are kept, as in the reference.
    pub(crate) fn reset(&mut self) {
        self.rng = 0;
        self.last_pitch_index = 0;
        self.loss_count = 0;
        self.postfilter_period = 0;
        self.postfilter_period_old = 0;
        self.postfilter_gain = 0.0;
        self.postfilter_gain_old = 0.0;
        self.postfilter_tapset = 0;
        self.postfilter_tapset_old = 0;
        self.preemph_mem = [0.0; 2];
        for m in &mut self.decode_mem {
            m.fill(0.0);
        }
        self.lpc = [[0.0; LPC_ORDER]; 2];
        self.old_band_e = [0.0; 2 * NB_EBANDS];
        self.old_log_e = [-28.0; 2 * NB_EBANDS];
        self.old_log_e2 = [-28.0; 2 * NB_EBANDS];
        self.background_log_e = [0.0; 2 * NB_EBANDS];
    }

    /// Decodes (`Some`) or conceals (`None`) one frame into `out` (`frame.frame_size *
    /// channels` interleaved samples), per the module's contract (`celt_decode_with_ec`).
    pub(crate) fn decode(
        &mut self,
        dec: Option<&mut RangeDecoder<'_>>,
        frame: &CeltFrame,
        out: &mut [f32],
    ) -> Result<()> {
        let lm = match frame.frame_size {
            120 => 0,
            240 => 1,
            480 => 2,
            960 => 3,
            _ => return Err(Error::BadArgument("a CELT frame is 2.5, 5, 10 or 20 ms")),
        };
        if frame.start_band >= NB_EBANDS
            || !(1..=NB_EBANDS).contains(&frame.end_band)
            || frame.start_band >= frame.end_band
            || !(1..=2).contains(&frame.stream_channels)
        {
            return Err(Error::BadArgument("CELT bands or channels out of range"));
        }
        if out.len() != frame.frame_size * self.channels {
            return Err(Error::BadArgument("CELT output of the wrong length"));
        }
        self.start = frame.start_band;
        self.end = frame.end_band;
        self.stream_channels = frame.stream_channels;
        match dec {
            Some(dec) if dec.storage() > 1 => {
                if dec.storage() > MAX_FRAME_BYTES {
                    return Err(Error::BadArgument("a CELT frame has at most 1275 bytes"));
                }
                self.decode_frame(dec, lm, out, frame.accumulate)
            }
            _ => {
                self.decode_lost(lm, out, frame.accumulate);
                Ok(())
            }
        }
    }

    fn decode_frame(
        &mut self,
        dec: &mut RangeDecoder<'_>,
        lm: usize,
        out: &mut [f32],
        accumulate: bool,
    ) -> Result<()> {
        let cc = self.channels;
        let c = self.stream_channels;
        let m = 1usize << lm;
        let n = m * SHORT_MDCT_SIZE;
        let start = self.start;
        let end = self.end;
        let eff_end = end.min(NB_EBANDS);
        let len = dec.storage() as i32;
        let mut ec = Ec::new(dec);

        if c == 1 {
            for i in 0..NB_EBANDS {
                self.old_band_e[i] = self.old_band_e[i].max(self.old_band_e[NB_EBANDS + i]);
            }
        }

        let mut total_bits = len * 8;
        let mut tell = ec.tell();
        let silence = if tell >= total_bits {
            true
        } else if tell == 1 {
            ec.bit_logp(15)
        } else {
            false
        };
        if silence {
            tell = len * 8;
            ec.pretend_read_to(tell);
        }

        let mut postfilter_gain = 0.0f32;
        let mut postfilter_pitch = 0i32;
        let mut postfilter_tapset = 0usize;
        if start == 0 && tell + 16 <= total_bits {
            if ec.bit_logp(1) {
                let octave = ec.uint(6);
                postfilter_pitch = ((16 << octave) + ec.bits(4 + octave)) as i32 - 1;
                let qg = ec.bits(3);
                if ec.tell() + 2 <= total_bits {
                    postfilter_tapset = ec.icdf(&TAPSET_ICDF, 2);
                }
                postfilter_gain = 0.093_75 * (qg + 1) as f32;
            }
            tell = ec.tell();
        }

        let transient = if lm > 0 && tell + 3 <= total_bits {
            let t = ec.bit_logp(3);
            tell = ec.tell();
            t
        } else {
            false
        };
        let intra = if tell + 3 <= total_bits {
            ec.bit_logp(3)
        } else {
            false
        };
        unquant_coarse_energy(&mut self.old_band_e, start, end, intra, &mut ec, c, lm);

        let mut tf_res = [0i32; NB_EBANDS];
        tf_decode(start, end, transient, &mut tf_res, lm, &mut ec);

        tell = ec.tell();
        let spread = if tell + 4 <= total_bits {
            ec.icdf(&SPREAD_ICDF, 5) as i32
        } else {
            SPREAD_NORMAL
        };

        let cap = init_caps(lm, c);
        let mut offsets = [0i32; NB_EBANDS];
        let mut dynalloc_logp = 6;
        total_bits <<= BITRES;
        let mut tell_frac = ec.tell_frac();
        for i in start..end {
            let width = (c as i32 * i32::from(EBANDS[i + 1] - EBANDS[i])) << lm;
            let quanta = (width << BITRES).min((6 << BITRES).max(width));
            let mut loop_logp = dynalloc_logp;
            let mut boost = 0;
            while tell_frac + (loop_logp << BITRES) < total_bits && boost < cap[i] {
                let flag = ec.bit_logp(loop_logp as u32);
                tell_frac = ec.tell_frac();
                if !flag {
                    break;
                }
                boost += quanta;
                total_bits -= quanta;
                loop_logp = 1;
            }
            offsets[i] = boost;
            if boost > 0 {
                dynalloc_logp = 2.max(dynalloc_logp - 1);
            }
        }

        let alloc_trim = if tell_frac + (6 << BITRES) <= total_bits {
            ec.icdf(&TRIM_ICDF, 7) as i32
        } else {
            5
        };

        let mut bits = ((len * 8) << BITRES) - ec.tell_frac() - 1;
        let anti_collapse_rsv = if transient && lm >= 2 && bits >= ((lm as i32 + 2) << BITRES) {
            1 << BITRES
        } else {
            0
        };
        bits -= anti_collapse_rsv;
        let alloc =
            compute_allocation(start, end, &offsets, &cap, alloc_trim, bits, c, lm, &mut ec);

        unquant_fine_energy(
            &mut self.old_band_e,
            start,
            end,
            &alloc.fine_quant,
            &mut ec,
            c,
        );

        let mut x = [0.0f32; 2 * 960];
        let mut collapse_masks = [0u8; 2 * NB_EBANDS];
        {
            let (xa, ya) = x.split_at_mut(n);
            let ya = &mut ya[..n];
            let all = AllBands {
                start,
                end,
                pulses: &alloc.pulses,
                short_blocks: transient,
                spread,
                dual_stereo: alloc.dual_stereo,
                intensity: alloc.intensity,
                tf_res: &tf_res,
                total_bits: len * (8 << BITRES) - anti_collapse_rsv,
                balance: alloc.balance,
                lm,
                coded_bands: alloc.coded_bands,
            };
            let y = if c == 2 { Some(ya) } else { None };
            quant_all_bands(&all, xa, y, &mut collapse_masks, &mut ec, &mut self.rng);
        }

        let anti_collapse_on = anti_collapse_rsv > 0 && ec.bits(1) != 0;

        let bits_left = len * 8 - ec.tell();
        unquant_energy_finalise(
            &mut self.old_band_e,
            start,
            end,
            &alloc.fine_quant,
            &alloc.fine_priority,
            bits_left,
            &mut ec,
            c,
        );

        if anti_collapse_on {
            anti_collapse(
                &mut x,
                &collapse_masks,
                lm,
                c,
                n,
                start,
                end,
                &self.old_band_e,
                &self.old_log_e,
                &self.old_log_e2,
                &alloc.pulses,
                self.rng,
            );
        }

        let mut band_e = [0.0f32; 2 * NB_EBANDS];
        log2_amp(&mut band_e, &self.old_band_e, start, end, c);
        if silence {
            band_e = [0.0; 2 * NB_EBANDS];
            self.old_band_e = [-28.0; 2 * NB_EBANDS];
        }

        let mut freq = [0.0f32; 2 * 960];
        for ch in 0..c {
            denormalise_bands(
                &x[ch * n..(ch + 1) * n],
                &mut freq[ch * n..(ch + 1) * n],
                &band_e[ch * NB_EBANDS..],
                eff_end,
                m,
            );
            freq[ch * n..ch * n + m * EBANDS[start] as usize].fill(0.0);
        }
        if cc == 2 && c == 1 {
            freq.copy_within(0..n, n);
        }
        if cc == 1 && c == 2 {
            for i in 0..n {
                freq[i] = 0.5 * (freq[i] + freq[n + i]);
            }
        }

        for mem in self.decode_mem.iter_mut().take(cc) {
            mem.copy_within(n..DECODE_BUFFER_SIZE, 0);
        }
        self.inverse_mdcts(&freq, transient, lm, n);

        let post = [
            self.postfilter_period.max(COMBFILTER_MINPERIOD),
            self.postfilter_period_old.max(COMBFILTER_MINPERIOD),
        ];
        self.postfilter_period = post[0];
        self.postfilter_period_old = post[1];
        let syn = DECODE_BUFFER_SIZE - n;
        for mem in self.decode_mem.iter_mut().take(cc) {
            comb_filter(
                mem,
                syn,
                self.postfilter_period_old,
                self.postfilter_period,
                SHORT_MDCT_SIZE,
                self.postfilter_gain_old,
                self.postfilter_gain,
                self.postfilter_tapset_old,
                self.postfilter_tapset,
                OVERLAP,
            );
            if lm != 0 {
                comb_filter(
                    mem,
                    syn + SHORT_MDCT_SIZE,
                    self.postfilter_period,
                    postfilter_pitch,
                    n - SHORT_MDCT_SIZE,
                    self.postfilter_gain,
                    postfilter_gain,
                    self.postfilter_tapset,
                    postfilter_tapset,
                    OVERLAP,
                );
            }
        }
        self.postfilter_period_old = self.postfilter_period;
        self.postfilter_gain_old = self.postfilter_gain;
        self.postfilter_tapset_old = self.postfilter_tapset;
        self.postfilter_period = postfilter_pitch;
        self.postfilter_gain = postfilter_gain;
        self.postfilter_tapset = postfilter_tapset;
        if lm != 0 {
            self.postfilter_period_old = self.postfilter_period;
            self.postfilter_gain_old = self.postfilter_gain;
            self.postfilter_tapset_old = self.postfilter_tapset;
        }

        if c == 1 {
            self.old_band_e.copy_within(0..NB_EBANDS, NB_EBANDS);
        }
        if !transient {
            self.old_log_e2 = self.old_log_e;
            self.old_log_e = self.old_band_e;
            for (b, &e) in self.background_log_e.iter_mut().zip(&self.old_band_e) {
                *b = (*b + m as f32 * 0.001).min(e);
            }
        } else {
            for (l, &e) in self.old_log_e.iter_mut().zip(&self.old_band_e) {
                *l = l.min(e);
            }
        }
        for ch in 0..2 {
            for i in (0..start).chain(end..NB_EBANDS) {
                self.old_band_e[ch * NB_EBANDS + i] = 0.0;
                self.old_log_e[ch * NB_EBANDS + i] = -28.0;
                self.old_log_e2[ch * NB_EBANDS + i] = -28.0;
            }
        }
        self.rng = ec.rng();

        self.deemphasis(out, n, accumulate);
        self.loss_count = 0;
        if ec.tell() > 8 * len {
            return Err(Error::Corrupt("CELT read past the end of its frame"));
        }
        Ok(())
    }

    /// `compute_inv_mdcts` into each channel's history, ending at `DECODE_BUFFER_SIZE`, with
    /// the overlap kept after it.
    fn inverse_mdcts(&mut self, freq: &[f32], transient: bool, lm: usize, n: usize) {
        let (n2, blocks, shift) = if transient {
            (SHORT_MDCT_SIZE, 1usize << lm, MAX_LM)
        } else {
            (n, 1, MAX_LM - lm)
        };
        let mut x = [0.0f32; 960 + OVERLAP];
        for (ch, mem) in self.decode_mem.iter_mut().take(self.channels).enumerate() {
            let x = &mut x[..n + OVERLAP];
            x[..OVERLAP].fill(0.0);
            let spec = &freq[ch * n..(ch + 1) * n];
            for b in 0..blocks {
                mdct::backward(&spec[b..], blocks, x, (n2 * b) as isize, shift);
            }
            let syn = DECODE_BUFFER_SIZE - n;
            for j in 0..OVERLAP {
                mem[syn + j] = x[j] + mem[DECODE_BUFFER_SIZE + j];
            }
            mem[syn + OVERLAP..DECODE_BUFFER_SIZE].copy_from_slice(&x[OVERLAP..n]);
            mem[DECODE_BUFFER_SIZE..].copy_from_slice(&x[n..n + OVERLAP]);
        }
    }

    /// `deemphasis`: the last `n` samples of history to `out`, interleaved, scaled to 1.0
    /// full scale; added to `out` when accumulating.
    fn deemphasis(&mut self, out: &mut [f32], n: usize, accumulate: bool) {
        let cc = self.channels;
        for ch in 0..cc {
            let mem = &self.decode_mem[ch][DECODE_BUFFER_SIZE - n..DECODE_BUFFER_SIZE];
            let mut m = self.preemph_mem[ch];
            for (j, &x) in mem.iter().enumerate() {
                let tmp = x + m;
                m = PREEMPH * tmp;
                let y = tmp * (1.0 / 32768.0);
                let o = &mut out[j * cc + ch];
                if accumulate {
                    *o += y;
                } else {
                    *o = y;
                }
            }
            self.preemph_mem[ch] = m;
        }
    }

    /// `celt_decode_lost`: conceals one frame of `120 << lm` samples.
    fn decode_lost(&mut self, lm: usize, out: &mut [f32], accumulate: bool) {
        let cc = self.channels;
        let m = 1usize << lm;
        let n = m * SHORT_MDCT_SIZE;
        let start = self.start;
        let end = self.end;
        let overlap = OVERLAP;

        if self.loss_count >= 5 || start != 0 {
            let eff_end = end.min(NB_EBANDS);
            let mut band_e = [0.0f32; 2 * NB_EBANDS];
            if self.loss_count >= 5 {
                log2_amp(&mut band_e, &self.background_log_e, start, end, cc);
            } else {
                let decay = if self.loss_count == 0 { 1.5 } else { 0.5 };
                for ch in 0..cc {
                    for i in start..end {
                        self.old_band_e[ch * NB_EBANDS + i] -= decay;
                    }
                }
                log2_amp(&mut band_e, &self.old_band_e, start, end, cc);
            }
            let mut seed = self.rng;
            let mut x = [0.0f32; 2 * 960];
            for ch in 0..cc {
                let xc = &mut x[ch * n..(ch + 1) * n];
                for i in start..NB_EBANDS {
                    let lo = (EBANDS[i] as usize) << lm;
                    let hi = (EBANDS[i + 1] as usize) << lm;
                    for v in &mut xc[lo..hi] {
                        seed = lcg_rand(seed);
                        *v = ((seed as i32) >> 20) as f32;
                    }
                    renormalise_vector(&mut xc[lo..hi], 1.0);
                }
                xc[(EBANDS[end] as usize) << lm..].fill(0.0);
            }
            self.rng = seed;
            let mut freq = [0.0f32; 2 * 960];
            for ch in 0..cc {
                denormalise_bands(
                    &x[ch * n..(ch + 1) * n],
                    &mut freq[ch * n..(ch + 1) * n],
                    &band_e[ch * NB_EBANDS..],
                    NB_EBANDS,
                    m,
                );
                freq[ch * n..ch * n + ((EBANDS[start] as usize) << lm)].fill(0.0);
                freq[ch * n + ((EBANDS[eff_end] as usize) << lm)..(ch + 1) * n].fill(0.0);
            }
            for mem in self.decode_mem.iter_mut().take(cc) {
                mem.copy_within(n..DECODE_BUFFER_SIZE, 0);
            }
            self.inverse_mdcts(&freq, false, lm, n);
        } else {
            let fade;
            let pitch_index;
            if self.loss_count == 0 {
                let mut pitch_buf = vec![0.0f32; DECODE_BUFFER_SIZE >> 1];
                let mems = [
                    &self.decode_mem[0][..DECODE_BUFFER_SIZE],
                    &self.decode_mem[cc - 1][..DECODE_BUFFER_SIZE],
                ];
                pitch_downsample(mems, cc, &mut pitch_buf);
                let poffset = 720;
                let found = pitch_search(
                    &pitch_buf[poffset >> 1..],
                    &pitch_buf,
                    DECODE_BUFFER_SIZE - poffset,
                    poffset - 100,
                );
                pitch_index = poffset.saturating_sub(found).clamp(1, MAX_PERIOD);
                self.last_pitch_index = pitch_index;
                fade = 1.0f32;
            } else {
                pitch_index = self.last_pitch_index.clamp(1, MAX_PERIOD);
                fade = 0.8f32;
            }
            let len = n + overlap;
            for ch in 0..cc {
                self.conceal_channel(ch, pitch_index, fade, n, len);
            }
        }
        self.deemphasis(out, n, accumulate);
        self.loss_count = self.loss_count.saturating_add(1);
    }

    /// The pitch-based concealment of one channel: the LPC excitation of the last pitch
    /// period, repeated with decay, synthesised and cross-faded with TDAC.
    fn conceal_channel(&mut self, ch: usize, pitch_index: usize, fade: f32, n: usize, len: usize) {
        let overlap = OVERLAP;
        // out_mem[i] is decode_mem[DECODE_BUFFER_SIZE - MAX_PERIOD + i].
        let om = DECODE_BUFFER_SIZE - MAX_PERIOD;
        let loss_count = self.loss_count;
        let mem = &mut self.decode_mem[ch];
        let lpc_c = &mut self.lpc[ch];
        let mut exc = vec![0.0f32; MAX_PERIOD];
        exc.copy_from_slice(&mem[om..om + MAX_PERIOD]);
        if loss_count == 0 {
            let mut ac = [0.0f32; LPC_ORDER + 1];
            autocorr(&exc, &mut ac, Some(&WINDOW));
            ac[0] *= 1.0001;
            for (i, a) in ac.iter_mut().enumerate().skip(1) {
                *a -= *a * (0.008 * i as f32) * (0.008 * i as f32);
            }
            lpc(lpc_c, &ac);
        }
        let mut fmem = [0.0f32; LPC_ORDER];
        for (i, f) in fmem.iter_mut().enumerate() {
            *f = mem[om + MAX_PERIOD - 1 - i];
        }
        fir(&mut exc, lpc_c, &mut fmem);
        let mut decay;
        {
            let mut e1 = 1.0f32;
            let mut e2 = 1.0f32;
            let period = pitch_index.min(MAX_PERIOD / 2);
            for i in 0..period {
                let a = exc[MAX_PERIOD - period + i];
                let b = exc[MAX_PERIOD - 2 * period + i];
                e1 += a * a;
                e2 += b * b;
            }
            if e1 > e2 {
                e1 = e2;
            }
            // SHR(E1, 1) is a no-op in the float build.
            decay = (e1 / e2).sqrt();
        }
        let mut e = vec![0.0f32; MAX_PERIOD + 2 * overlap];
        let mut s1 = 0.0f32;
        let mut offset = (MAX_PERIOD - pitch_index) as isize;
        for i in 0..len + overlap {
            if offset + i as isize >= MAX_PERIOD as isize {
                offset -= pitch_index as isize;
                decay *= decay;
            }
            let at = (offset + i as isize) as usize;
            e[i] = decay * exc[at];
            let tmp = mem[om + at];
            s1 += tmp * tmp;
        }
        for (i, f) in fmem.iter_mut().enumerate() {
            *f = mem[om + MAX_PERIOD - 1 - i];
        }
        for v in &mut e[..len + overlap] {
            *v *= fade;
        }
        iir(&mut e[..len + overlap], lpc_c, &mut fmem);
        {
            let mut s2 = 0.0f32;
            for &v in &e[..len + overlap] {
                s2 += v * v;
            }
            // Written so that a NaN energy also mutes, as the reference's test does.
            if s1.partial_cmp(&(0.2 * s2)) != Some(std::cmp::Ordering::Greater) {
                e[..len + overlap].fill(0.0);
            } else if s1 < s2 {
                let ratio = ((s1 + 1.0) / (s2 + 1.0)).sqrt();
                for v in &mut e[..len + overlap] {
                    *v *= ratio;
                }
            }
        }
        let (period, gain, tapset) = (
            self.postfilter_period.max(COMBFILTER_MINPERIOD),
            self.postfilter_gain,
            self.postfilter_tapset,
        );
        // Post-filter the previous frame's overlap (window NULL, overlap 0: steady taps).
        let (g10, g11, g12) = (
            gain * COMB_GAINS[tapset][0],
            gain * COMB_GAINS[tapset][1],
            gain * COMB_GAINS[tapset][2],
        );
        comb_filter_const(
            mem,
            om + MAX_PERIOD,
            period as usize,
            overlap,
            g10,
            g11,
            g12,
        );
        mem.copy_within(n..DECODE_BUFFER_SIZE + overlap, 0);
        for i in 0..overlap / 2 {
            let tmp = WINDOW[i] * e[n + overlap - 1 - i] + WINDOW[overlap - i - 1] * e[n + i];
            mem[om + MAX_PERIOD + i] = WINDOW[overlap - i - 1] * tmp;
            mem[om + MAX_PERIOD + overlap - i - 1] = WINDOW[i] * tmp;
        }
        mem[om + MAX_PERIOD - n..om + MAX_PERIOD].copy_from_slice(&e[..n]);
        // Pre-filter the overlap for the next frame (its post-filter undoes this).
        let mut pre = vec![0.0f32; MAX_PERIOD + overlap];
        pre[..MAX_PERIOD + overlap].copy_from_slice(&mem[om..om + MAX_PERIOD + overlap]);
        let mut ov = [0.0f32; OVERLAP];
        for i in 0..overlap {
            let x = MAX_PERIOD + i;
            let t = period as usize;
            ov[i] = pre[x]
                + -g10 * pre[x - t]
                + -g11 * pre[x - t - 1]
                + -g11 * pre[x - t + 1]
                + -g12 * pre[x - t - 2]
                + -g12 * pre[x - t + 2];
        }
        mem[om + MAX_PERIOD..om + MAX_PERIOD + overlap].copy_from_slice(&ov);
    }
}

#[cfg(test)]
#[path = "decoder_tests.rs"]
mod tests;
