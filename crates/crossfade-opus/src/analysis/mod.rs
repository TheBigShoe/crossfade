//! The tonality and speech/music analysis of libopus 1.5.2's float encoder (`src/analysis.c`,
//! 3-clause BSD, see the crate's `COPYING`): every 20 ms of the downmixed input, resampled to
//! 24 kHz with SILK's 2:1 all-pass, goes through a windowed 480-point FFT; the phase
//! predictability of each bin gives its tonality, the band energies give the activity, the
//! bandwidth, the leakage boosts and the cepstral features of a small recurrent network
//! (`mlp.rs`), whose outputs are the probabilities of music and of activity. The results wait
//! in a ring buffer that [`TonalityAnalysis::get_info`] reads with a little look-ahead and
//! smoothing, as `tonality_get_info` does.
//!
//! Arithmetic as in the reference's float build: `log`, `log10` and `sqrt` in double where it
//! takes them in double (through `libm`), its approximations of `atan2` and `tanh`, no
//! `mul_add`; so the analysis is the same bits on every platform, and within rounding of
//! libopus's (whose FFT differs from ours in its last bits).
#![allow(clippy::needless_range_loop)]
// index loops as the reference writes them
// 1.442695 is the reference's rounded log2(e), not LOG2_E.
#![allow(clippy::approx_constant)]

mod mlp;
mod mlp_data;
mod tables;

use crate::celt::{Cpx, fft};
use mlp::Mlp;
use tables::{ANALYSIS_WINDOW, DCT_TABLE};

/// The bands whose leakage boost the analysis reports (`LEAK_BANDS`).
pub(crate) const LEAK_BANDS: usize = 19;
/// Frames of band energies kept for stationarity and spectral variability.
const NB_FRAMES: usize = 8;
const NB_TBANDS: usize = 18;
/// 30 ms at 24 kHz.
const ANALYSIS_BUF_SIZE: usize = 720;
const ANALYSIS_COUNT_MAX: i32 = 10_000;
/// The ring of results.
const DETECT_SIZE: usize = 100;
const TRANSITION_PENALTY: f32 = 10.0;
const NB_TONAL_SKIP_BANDS: usize = 9;
const LEAKAGE_OFFSET: f32 = 2.5;
const LEAKAGE_SLOPE: f32 = 2.0;
/// The depth the float API claims for its input (`lsb_depth`).
const LSB_DEPTH: i32 = 24;
const TBANDS: [usize; NB_TBANDS + 1] = [
    4, 8, 12, 16, 20, 24, 28, 32, 40, 48, 56, 64, 80, 96, 112, 136, 160, 192, 240,
];
const STD_FEATURE_BIAS: [f32; 9] = [
    5.684_947, 3.475_288, 1.770_634, 1.599_784, 3.773_215, 2.163_313, 1.260_756, 1.116_868,
    1.918_795,
];

/// What the analysis says of a frame (`AnalysisInfo`), for the Opus layer's decisions and
/// CELT's allocation.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct AnalysisInfo {
    pub(crate) valid: bool,
    pub(crate) tonality: f32,
    pub(crate) tonality_slope: f32,
    pub(crate) noisiness: f32,
    pub(crate) activity: f32,
    pub(crate) music_prob: f32,
    pub(crate) music_prob_min: f32,
    pub(crate) music_prob_max: f32,
    /// The highest active band, 0 to 20 (of the 21 CELT bands; 20 is fullband).
    pub(crate) bandwidth: i32,
    pub(crate) activity_probability: f32,
    pub(crate) max_pitch_ratio: f32,
    /// Per band, in 1/64 dB-ish steps (Q6 of CELT's log2 energy).
    pub(crate) leak_boost: [u8; LEAK_BANDS],
}

/// The C macros, whose NaN behaviour `f32::min` and `max` do not share.
fn min16(a: f32, b: f32) -> f32 {
    if a < b { a } else { b }
}

fn max16(a: f32, b: f32) -> f32 {
    if a > b { a } else { b }
}

/// `fast_atan2f`: within about 1e-5 rad, 0 for a vanishing vector.
#[allow(clippy::excessive_precision)] // the reference's literals
fn fast_atan2f(y: f32, x: f32) -> f32 {
    const CA: f32 = 0.431_579_74;
    const CB: f32 = 0.678_484_03;
    const CC: f32 = 0.085_955_42;
    const CE: f32 = std::f32::consts::FRAC_PI_2;
    let x2 = x * x;
    let y2 = y * y;
    if x2 + y2 < 1e-18 {
        return 0.0;
    }
    if x2 < y2 {
        let den = (y2 + CB * x2) * (y2 + CC * x2);
        -x * y * (y2 + CA * x2) / den + if y < 0.0 { -CE } else { CE }
    } else {
        let den = (x2 + CB * y2) * (x2 + CC * y2);
        x * y * (x2 + CA * y2) / den + if y < 0.0 { -CE } else { CE }
            - if x * y < 0.0 { -CE } else { CE }
    }
}

/// `celt_maxabs16`, NaNs and all.
pub(crate) fn maxabs(x: &[f32]) -> f32 {
    let (mut maxval, mut minval) = (0.0f32, 0.0f32);
    for &v in x {
        maxval = max16(maxval, v);
        minval = min16(minval, v);
    }
    max16(maxval, -minval)
}

/// `log` of a float in double, back to float, as the reference's `(float)log(x)`.
fn log_d(x: f32) -> f32 {
    libm::log(f64::from(x)) as f32
}

/// `silk_resampler_down2_hp`: halves the rate of `input` into `out` with SILK's two all-pass
/// sections, and returns the energy of the high band it removed.
fn resampler_down2_hp(s: &mut [f32; 3], out: &mut [f32], input: &[f32]) -> f32 {
    let mut hp_ener = 0.0f32;
    for (k, o) in out.iter_mut().enumerate().take(input.len() / 2) {
        let in32 = input[2 * k];
        let y = in32 - s[0];
        let x = 0.607_437_1 * y;
        let mut out32 = s[0] + x;
        s[0] = in32 + x;
        let mut out32_hp = out32;
        let in32 = input[2 * k + 1];
        let y = in32 - s[1];
        let x = 0.150_63 * y;
        out32 += s[1];
        out32 += x;
        s[1] = in32 + x;
        let y = -in32 - s[2];
        let x = 0.150_63 * y;
        out32_hp += s[2];
        out32_hp += x;
        s[2] = -in32 + x;
        hp_ener += out32_hp * out32_hp;
        *o = 0.5 * out32;
    }
    hp_ener
}

/// The analyser's state (`TonalityAnalysisState` at 48 kHz).
#[derive(Debug, Clone)]
pub(crate) struct TonalityAnalysis {
    angle: [f32; 240],
    d_angle: [f32; 240],
    d2_angle: [f32; 240],
    inmem: [f32; ANALYSIS_BUF_SIZE],
    /// Samples (at 24 kHz) of `inmem` in use.
    mem_fill: usize,
    prev_band_tonality: [f32; NB_TBANDS],
    prev_tonality: f32,
    prev_bandwidth: i32,
    e: [[f32; NB_TBANDS]; NB_FRAMES],
    log_e: [[f32; NB_TBANDS]; NB_FRAMES],
    low_e: [f32; NB_TBANDS],
    high_e: [f32; NB_TBANDS],
    mean_e: [f32; NB_TBANDS + 1],
    mem: [f32; 32],
    cmean: [f32; 8],
    std: [f32; 9],
    e_tracker: f32,
    low_e_count: f32,
    e_count: usize,
    count: i32,
    /// Samples (at 48 kHz) of the last call's input already analysed.
    analysis_offset: usize,
    write_pos: usize,
    read_pos: usize,
    read_subframe: usize,
    hp_ener_accum: f32,
    initialized: bool,
    mlp: Mlp,
    downmix_state: [f32; 3],
    info: [AnalysisInfo; DETECT_SIZE],
}

impl Default for TonalityAnalysis {
    fn default() -> Self {
        Self {
            angle: [0.0; 240],
            d_angle: [0.0; 240],
            d2_angle: [0.0; 240],
            inmem: [0.0; ANALYSIS_BUF_SIZE],
            mem_fill: 0,
            prev_band_tonality: [0.0; NB_TBANDS],
            prev_tonality: 0.0,
            prev_bandwidth: 0,
            e: [[0.0; NB_TBANDS]; NB_FRAMES],
            log_e: [[0.0; NB_TBANDS]; NB_FRAMES],
            low_e: [0.0; NB_TBANDS],
            high_e: [0.0; NB_TBANDS],
            mean_e: [0.0; NB_TBANDS + 1],
            mem: [0.0; 32],
            cmean: [0.0; 8],
            std: [0.0; 9],
            e_tracker: 0.0,
            low_e_count: 0.0,
            e_count: 0,
            count: 0,
            analysis_offset: 0,
            write_pos: 0,
            read_pos: 0,
            read_subframe: 0,
            hp_ener_accum: 0.0,
            initialized: false,
            mlp: Mlp::default(),
            downmix_state: [0.0; 3],
            info: [AnalysisInfo::default(); DETECT_SIZE],
        }
    }
}

impl TonalityAnalysis {
    /// Forgets everything (`tonality_analysis_reset`).
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    /// Where [`get_info`](Self::get_info) reads next, to read a packet's frames one by one.
    pub(crate) fn read_position(&self) -> (usize, usize) {
        (self.read_pos, self.read_subframe)
    }

    pub(crate) fn set_read_position(&mut self, (pos, subframe): (usize, usize)) {
        self.read_pos = pos;
        self.read_subframe = subframe;
    }

    /// `run_analysis` for 48 kHz input: analyses `pcm` (`frame_size` samples of `channels`,
    /// interleaved, full scale 1.0, mixed down to mono) in 20 ms steps, then reads the
    /// result for the frame.
    pub(crate) fn run(
        &mut self,
        pcm: &[f32],
        channels: usize,
        frame_size: usize,
        lsb_depth: u8,
    ) -> AnalysisInfo {
        let analysis_frame_size = frame_size - (frame_size & 1);
        let analysis_frame_size = analysis_frame_size.min((DETECT_SIZE - 5) * 960);
        let mut pcm_len = analysis_frame_size as i64 - self.analysis_offset as i64;
        let mut offset = self.analysis_offset;
        while pcm_len > 0 {
            self.tonality_analysis(
                pcm,
                channels,
                (pcm_len as usize).min(960),
                offset,
                i32::from(lsb_depth),
            );
            offset += 960;
            pcm_len -= 960;
        }
        self.analysis_offset = analysis_frame_size.saturating_sub(frame_size);
        self.get_info(frame_size)
    }

    /// `downmix_and_resample`: `subframe` samples (at 24 kHz) of the mono downmix from
    /// `offset` (at 24 kHz) into `inmem` at `at`; returns the energy above 12 kHz.
    fn downmix_and_resample(
        &mut self,
        pcm: &[f32],
        channels: usize,
        subframe: usize,
        offset: usize,
        at: usize,
    ) -> f32 {
        if subframe == 0 {
            return 0.0;
        }
        let subframe = subframe * 2;
        let offset = offset * 2;
        let mut tmp = [0.0f32; 960];
        let tmp = &mut tmp[..subframe];
        for (j, t) in tmp.iter_mut().enumerate() {
            let frame = &pcm[(j + offset) * channels..(j + offset + 1) * channels];
            // PCM2VAL: the float build scales to 16-bit range.
            *t = frame[0] * 32768.0;
            for &v in &frame[1..] {
                *t += v * 32768.0;
            }
        }
        let scale = (1.0f32 / 32768.0) / channels as f32;
        for t in tmp.iter_mut() {
            *t *= scale;
        }
        resampler_down2_hp(
            &mut self.downmix_state,
            &mut self.inmem[at..at + subframe / 2],
            tmp,
        )
    }

    /// `tonality_analysis`: buffers `len` samples (at 48 kHz) of `pcm` from `offset`, and
    /// analyses a frame once 30 ms (at 24 kHz) are in.
    fn tonality_analysis(
        &mut self,
        pcm: &[f32],
        channels: usize,
        len: usize,
        offset: usize,
        lsb_depth: i32,
    ) {
        const N: usize = 480;
        const N2: usize = 240;
        if !self.initialized {
            self.mem_fill = 240;
            self.initialized = true;
        }
        let alpha = 1.0 / (10.min(1 + self.count)) as f32;
        let alpha_e = 1.0 / (25.min(1 + self.count)) as f32;
        // The noise floor's decay for the bandwidth detection: -2.2 dB/s.
        let mut alpha_e2 = 1.0 / (100.min(1 + self.count)) as f32;
        if self.count <= 1 {
            alpha_e2 = 1.0;
        }
        // At 24 kHz from here on.
        let len = len / 2;
        let offset = offset / 2;
        let take = len.min(ANALYSIS_BUF_SIZE - self.mem_fill);
        self.hp_ener_accum += self.downmix_and_resample(pcm, channels, take, offset, self.mem_fill);
        if self.mem_fill + len < ANALYSIS_BUF_SIZE {
            self.mem_fill += len;
            return;
        }
        let hp_ener = self.hp_ener_accum;
        let pos = self.write_pos;
        self.write_pos += 1;
        if self.write_pos >= DETECT_SIZE {
            self.write_pos -= DETECT_SIZE;
        }

        let sample_max = maxabs(&self.inmem);
        let is_silence = sample_max <= 1.0 / (1 << lsb_depth) as f32;

        let mut input = [Cpx::default(); N];
        for i in 0..N2 {
            let w = ANALYSIS_WINDOW[i];
            input[i].r = w * self.inmem[i];
            input[i].i = w * self.inmem[N2 + i];
            input[N - i - 1].r = w * self.inmem[N - i - 1];
            input[N - i - 1].i = w * self.inmem[N + N2 - i - 1];
        }
        self.inmem.copy_within(ANALYSIS_BUF_SIZE - 240.., 0);
        let remaining = len - (ANALYSIS_BUF_SIZE - self.mem_fill);
        self.hp_ener_accum = self.downmix_and_resample(
            pcm,
            channels,
            remaining,
            offset + ANALYSIS_BUF_SIZE - self.mem_fill,
            240,
        );
        self.mem_fill = 240 + remaining;
        if is_silence {
            // On silence, the previous analysis again.
            let prev = (pos + DETECT_SIZE - 1) % DETECT_SIZE;
            self.info[pos] = self.info[prev];
            return;
        }
        let mut out = [Cpx::default(); N];
        fft(0, &input, &mut out);
        // A NaN anywhere in the input makes every output NaN.
        if out[0].r.is_nan() {
            self.info[pos].valid = false;
            return;
        }

        let mut tonality = [0.0f32; 240];
        let mut noisiness = [0.0f32; 240];
        let mut tonality2 = [0.0f32; 240];
        let pi = std::f64::consts::PI;
        let pi4 = (pi * pi * pi * pi) as f32;
        let half_over_pi = (0.5 / std::f64::consts::PI) as f32;
        for i in 1..N2 {
            let x1r = out[i].r + out[N - i].r;
            let x1i = out[i].i - out[N - i].i;
            let x2r = out[i].i + out[N - i].i;
            let x2i = out[N - i].r - out[i].r;

            let angle = half_over_pi * fast_atan2f(x1i, x1r);
            let d_angle = angle - self.angle[i];
            let d2_angle = d_angle - self.d_angle[i];

            let angle2 = half_over_pi * fast_atan2f(x2i, x2r);
            let d_angle2 = angle2 - angle;
            let d2_angle2 = d_angle2 - d_angle;

            let mut mod1 = d2_angle - d2_angle.round_ties_even();
            noisiness[i] = mod1.abs();
            mod1 *= mod1;
            mod1 *= mod1;

            let mut mod2 = d2_angle2 - d2_angle2.round_ties_even();
            noisiness[i] += mod2.abs();
            mod2 *= mod2;
            mod2 *= mod2;

            let avg_mod = 0.25 * (self.d2_angle[i] + mod1 + 2.0 * mod2);
            // Two frames of delay in this one.
            tonality[i] = 1.0 / (1.0 + 40.0 * 16.0 * pi4 * avg_mod) - 0.015;
            // None in this one, but it is less reliable.
            tonality2[i] = 1.0 / (1.0 + 40.0 * 16.0 * pi4 * mod2) - 0.015;

            self.angle[i] = angle2;
            self.d_angle[i] = d_angle2;
            self.d2_angle[i] = mod2;
        }
        for i in 2..N2 - 1 {
            let tt = min16(tonality2[i], max16(tonality2[i - 1], tonality2[i + 1]));
            tonality[i] = 0.9 * max16(tonality[i], tt - 0.1);
        }

        let mut frame_tonality = 0.0f32;
        let mut max_frame_tonality = 0.0f32;
        let mut frame_noisiness = 0.0f32;
        let mut frame_stationarity = 0.0f32;
        self.info[pos].activity = 0.0;
        if self.count == 0 {
            self.low_e = [1e10; NB_TBANDS];
            self.high_e = [-1e10; NB_TBANDS];
        }
        let mut relative_e = 0.0f32;
        let mut frame_loudness = 0.0f32;
        let mut band_log2 = [0.0f32; NB_TBANDS + 1];
        let bin_e = |i: usize| {
            out[i].r * out[i].r
                + out[N - i].r * out[N - i].r
                + out[i].i * out[i].i
                + out[N - i].i * out[N - i].i
        };
        {
            // The first band's energy is special: DC.
            let x1r = 2.0 * out[0].r;
            let x2r = 2.0 * out[0].i;
            let mut e = x1r * x1r + x2r * x2r;
            for i in 1..4 {
                e += bin_e(i);
            }
            band_log2[0] = 0.5 * 1.442_695 * log_d(e + 1e-10);
        }
        let mut log_e = [0.0f32; NB_TBANDS];
        let mut band_tonality = [0.0f32; NB_TBANDS];
        let mut slope = 0.0f32;
        for b in 0..NB_TBANDS {
            let (mut e, mut t_e, mut n_e) = (0.0f32, 0.0f32, 0.0f32);
            for i in TBANDS[b]..TBANDS[b + 1] {
                let be = bin_e(i);
                e += be;
                t_e += be * max16(0.0, tonality[i]);
                n_e += be * 2.0 * (0.5 - noisiness[i]);
            }
            // Extreme energies would make NaNs further on.
            if e.partial_cmp(&1e9) != Some(std::cmp::Ordering::Less) {
                self.info[pos].valid = false;
                return;
            }
            self.e[self.e_count][b] = e;
            frame_noisiness += n_e / (1e-15 + e);
            frame_loudness += (e + 1e-10).sqrt();
            log_e[b] = log_d(e + 1e-10);
            band_log2[b + 1] = 0.5 * 1.442_695 * log_d(e + 1e-10);
            self.log_e[self.e_count][b] = log_e[b];
            if self.count == 0 {
                self.high_e[b] = log_e[b];
                self.low_e[b] = log_e[b];
            }
            if f64::from(self.high_e[b]) > f64::from(self.low_e[b]) + 7.5 {
                if self.high_e[b] - log_e[b] > log_e[b] - self.low_e[b] {
                    self.high_e[b] -= 0.01;
                } else {
                    self.low_e[b] += 0.01;
                }
            }
            if log_e[b] > self.high_e[b] {
                self.high_e[b] = log_e[b];
                self.low_e[b] = max16(self.high_e[b] - 15.0, self.low_e[b]);
            } else if log_e[b] < self.low_e[b] {
                self.low_e[b] = log_e[b];
                self.high_e[b] = min16(self.low_e[b] + 15.0, self.high_e[b]);
            }
            relative_e += (log_e[b] - self.low_e[b]) / (1e-5 + (self.high_e[b] - self.low_e[b]));

            let (mut l1, mut l2) = (0.0f32, 0.0f32);
            for i in 0..NB_FRAMES {
                l1 += self.e[i][b].sqrt();
                l2 += self.e[i][b];
            }
            let denom = libm::sqrt(1e-15 + f64::from(NB_FRAMES as f32 * l2)) as f32;
            let mut stationarity = min16(0.99, l1 / denom);
            stationarity *= stationarity;
            stationarity *= stationarity;
            frame_stationarity += stationarity;
            band_tonality[b] = max16(t_e / (1e-15 + e), stationarity * self.prev_band_tonality[b]);
            frame_tonality += band_tonality[b];
            if b >= NB_TBANDS - NB_TONAL_SKIP_BANDS {
                frame_tonality -= band_tonality[b + NB_TONAL_SKIP_BANDS - NB_TBANDS];
            }
            max_frame_tonality = max16(
                max_frame_tonality,
                (1.0 + 0.03 * (b as i32 - NB_TBANDS as i32) as f32) * frame_tonality,
            );
            slope += band_tonality[b] * (b as i32 - 8) as f32;
            self.prev_band_tonality[b] = band_tonality[b];
        }

        let mut leakage_from = [0.0f32; NB_TBANDS + 1];
        let mut leakage_to = [0.0f32; NB_TBANDS + 1];
        leakage_from[0] = band_log2[0];
        leakage_to[0] = band_log2[0] - LEAKAGE_OFFSET;
        for b in 1..NB_TBANDS + 1 {
            let leak_slope = LEAKAGE_SLOPE * (TBANDS[b] - TBANDS[b - 1]) as f32 / 4.0;
            leakage_from[b] = min16(leakage_from[b - 1] + leak_slope, band_log2[b]);
            leakage_to[b] = max16(
                leakage_to[b - 1] - leak_slope,
                band_log2[b] - LEAKAGE_OFFSET,
            );
        }
        for b in (0..NB_TBANDS - 1).rev() {
            let leak_slope = LEAKAGE_SLOPE * (TBANDS[b + 1] - TBANDS[b]) as f32 / 4.0;
            leakage_from[b] = min16(leakage_from[b + 1] + leak_slope, leakage_from[b]);
            leakage_to[b] = max16(leakage_to[b + 1] - leak_slope, leakage_to[b]);
        }
        for b in 0..NB_TBANDS + 1 {
            // Boosts against the analysis leakage from louder neighbours into band b, and
            // against the quantisation noise of a loud band b leaking into weaker ones.
            let boost = max16(0.0, leakage_to[b] - band_log2[b])
                + max16(0.0, band_log2[b] - (leakage_from[b] + LEAKAGE_OFFSET));
            let q = libm::floor(0.5 + f64::from(64.0 * boost)) as i32;
            self.info[pos].leak_boost[b] = q.clamp(0, 255) as u8;
        }

        let mut spec_variability = 0.0f32;
        for i in 0..NB_FRAMES {
            let mut mindist = 1e15f32;
            for j in 0..NB_FRAMES {
                let mut dist = 0.0f32;
                for k in 0..NB_TBANDS {
                    let tmp = self.log_e[i][k] - self.log_e[j][k];
                    dist += tmp * tmp;
                }
                if j != i {
                    mindist = min16(mindist, dist);
                }
            }
            spec_variability += mindist;
        }
        spec_variability = (spec_variability / NB_FRAMES as f32 / NB_TBANDS as f32).sqrt();

        let mut bandwidth_mask = 0.0f32;
        let mut bandwidth = 0i32;
        let mut max_e = 0.0f32;
        let mut noise_floor = 5.7e-4f32 / (1 << (LSB_DEPTH - 8).max(0)) as f32;
        noise_floor *= noise_floor;
        let mut below_max_pitch = 0.0f32;
        let mut above_max_pitch = 0.0f32;
        let mut is_masked = [false; NB_TBANDS + 1];
        for b in 0..NB_TBANDS {
            let (band_start, band_end) = (TBANDS[b], TBANDS[b + 1]);
            let mut e = 0.0f32;
            for i in band_start..band_end {
                e += bin_e(i);
            }
            max_e = max16(max_e, e);
            if band_start < 64 {
                below_max_pitch += e;
            } else {
                above_max_pitch += e;
            }
            self.mean_e[b] = max16((1.0 - alpha_e2) * self.mean_e[b], e);
            let em = max16(e, self.mean_e[b]);
            // Active when no more than 90 dB below the loudest band (the most masking there
            // can be) and above the noise floor of the input's depth.
            let width = (band_end - band_start) as f32;
            if e * 1e9 > max_e && (em > 3.0 * noise_floor * width || e > noise_floor * width) {
                bandwidth = b as i32 + 1;
            }
            let threshold = if self.prev_bandwidth > b as i32 {
                0.01
            } else {
                0.05
            };
            is_masked[b] = e < threshold * bandwidth_mask;
            // A follower of 13 dB per Bark for the spreading.
            bandwidth_mask = max16(0.05 * bandwidth_mask, e);
        }
        {
            // The last two bands have only the energy above 12 kHz, and the high-pass leaks
            // some of the low band: a higher threshold.
            let e = hp_ener * (1.0 / (60.0 * 60.0));
            let noise_ratio = if self.prev_bandwidth == 20 {
                10.0
            } else {
                30.0
            };
            above_max_pitch += e;
            let b = NB_TBANDS;
            self.mean_e[b] = max16((1.0 - alpha_e2) * self.mean_e[b], e);
            let em = max16(e, self.mean_e[b]);
            if em > 3.0 * noise_ratio * noise_floor * 160.0 || e > noise_ratio * noise_floor * 160.0
            {
                bandwidth = 20;
            }
            let threshold = if self.prev_bandwidth == 20 {
                0.01
            } else {
                0.05
            };
            is_masked[b] = e < threshold * bandwidth_mask;
        }
        self.info[pos].max_pitch_ratio = if above_max_pitch > below_max_pitch {
            below_max_pitch / above_max_pitch
        } else {
            1.0
        };
        // Resampling aliasing can put a little energy in the first band cut off: a masked
        // last band does not count.
        if bandwidth == 20 && is_masked[NB_TBANDS] {
            bandwidth -= 2;
        } else if bandwidth > 0
            && bandwidth <= NB_TBANDS as i32
            && is_masked[bandwidth as usize - 1]
        {
            bandwidth -= 1;
        }
        if self.count <= 2 {
            bandwidth = 20;
        }
        frame_loudness = 20.0 * libm::log10(f64::from(frame_loudness)) as f32;
        self.e_tracker = max16(self.e_tracker - 0.003, frame_loudness);
        self.low_e_count *= 1.0 - alpha_e;
        if frame_loudness < self.e_tracker - 30.0 {
            self.low_e_count += alpha_e;
        }

        let mut bfcc = [0.0f32; 8];
        let mut mid_e = [0.0f32; 8];
        for i in 0..8 {
            let mut sum = 0.0f32;
            for b in 0..16 {
                sum += DCT_TABLE[i * 16 + b] * log_e[b];
            }
            bfcc[i] = sum;
            let mut sum = 0.0f32;
            for b in 0..16 {
                sum += DCT_TABLE[i * 16 + b] * 0.5 * (self.high_e[b] + self.low_e[b]);
            }
            mid_e[i] = sum;
        }

        frame_stationarity /= NB_TBANDS as f32;
        relative_e /= NB_TBANDS as f32;
        if self.count < 10 {
            relative_e = 0.5;
        }
        frame_noisiness /= NB_TBANDS as f32;
        let info = &mut self.info[pos];
        info.activity = frame_noisiness + (1.0 - frame_noisiness) * relative_e;
        let mut frame_tonality = max_frame_tonality / (NB_TBANDS - NB_TONAL_SKIP_BANDS) as f32;
        frame_tonality = max16(frame_tonality, self.prev_tonality * 0.8);
        self.prev_tonality = frame_tonality;

        slope /= (8 * 8) as f32;
        info.tonality_slope = slope;

        self.e_count = (self.e_count + 1) % NB_FRAMES;
        self.count = (self.count + 1).min(ANALYSIS_COUNT_MAX);
        info.tonality = frame_tonality;

        let mem = &mut self.mem;
        let mut features = [0.0f32; 25];
        for i in 0..4 {
            features[i] = -0.122_99 * (bfcc[i] + mem[i + 24])
                + 0.491_95 * (mem[i] + mem[i + 16])
                + 0.696_93 * mem[i + 8]
                - 1.434_9 * self.cmean[i];
        }
        for i in 0..4 {
            self.cmean[i] = (1.0 - alpha) * self.cmean[i] + alpha * bfcc[i];
        }
        for i in 0..4 {
            features[4 + i] =
                0.632_46 * (bfcc[i] - mem[i + 24]) + 0.316_23 * (mem[i] - mem[i + 16]);
        }
        for i in 0..3 {
            features[8 + i] = 0.534_52 * (bfcc[i] + mem[i + 24])
                - 0.267_26 * (mem[i] + mem[i + 16])
                - 0.534_52 * mem[i + 8];
        }
        if self.count > 5 {
            for i in 0..9 {
                self.std[i] = (1.0 - alpha) * self.std[i] + alpha * features[i] * features[i];
            }
        }
        for i in 0..4 {
            features[i] = bfcc[i] - mid_e[i];
        }
        for i in 0..8 {
            mem[i + 24] = mem[i + 16];
            mem[i + 16] = mem[i + 8];
            mem[i + 8] = mem[i];
            mem[i] = bfcc[i];
        }
        for i in 0..9 {
            features[11 + i] = self.std[i].sqrt() - STD_FEATURE_BIAS[i];
        }
        features[18] = spec_variability - 0.78;
        features[20] = info.tonality - 0.154_723;
        features[21] = info.activity - 0.724_643;
        features[22] = frame_stationarity - 0.743_717;
        features[23] = info.tonality_slope + 0.069_216;
        features[24] = self.low_e_count - 0.067_930;

        let [music, active] = self.mlp.run(&features);
        // The probabilities of speech or music against noise.
        info.activity_probability = active;
        info.music_prob = music;
        info.bandwidth = bandwidth;
        self.prev_bandwidth = bandwidth;
        info.noisiness = frame_noisiness;
        info.valid = true;
    }

    /// `tonality_get_info`: the analysis for the next `len` samples (at 48 kHz), with the
    /// look-ahead the ring holds: the highest bandwidth around, the tonality of the frames
    /// ahead, and the thresholds for switching between speech and music.
    pub(crate) fn get_info(&mut self, len: usize) -> AnalysisInfo {
        let ds = DETECT_SIZE as i64;
        let write_pos = self.write_pos as i64;
        let mut pos = self.read_pos as i64;
        let mut curr_lookahead = write_pos - pos;
        if curr_lookahead < 0 {
            curr_lookahead += ds;
        }
        self.read_subframe += len / 120;
        while self.read_subframe >= 8 {
            self.read_subframe -= 8;
            self.read_pos += 1;
        }
        if self.read_pos >= DETECT_SIZE {
            self.read_pos -= DETECT_SIZE;
        }
        // Long frames look at the second analysis window rather than the first.
        if len > 960 && pos != write_pos {
            pos += 1;
            if pos == ds {
                pos = 0;
            }
        }
        if pos == write_pos {
            pos -= 1;
        }
        if pos < 0 {
            pos = ds - 1;
        }
        let pos0 = pos;
        let mut out = self.info[pos as usize];
        if !out.valid {
            return out;
        }
        let mut tonality_max = out.tonality;
        let mut tonality_avg = out.tonality;
        let mut tonality_count = 1;
        // The widest bandwidth of the frames around, to be safe.
        let mut bandwidth_span = 6;
        // Look ahead for a tone, against the tone detector's delay.
        for _ in 0..3 {
            pos += 1;
            if pos == ds {
                pos = 0;
            }
            if pos == write_pos {
                break;
            }
            let i = &self.info[pos as usize];
            tonality_max = max16(tonality_max, i.tonality);
            tonality_avg += i.tonality;
            tonality_count += 1;
            out.bandwidth = out.bandwidth.max(i.bandwidth);
            bandwidth_span -= 1;
        }
        pos = pos0;
        // And back for a wider bandwidth than this frame's.
        for _ in 0..bandwidth_span {
            pos -= 1;
            if pos < 0 {
                pos = ds - 1;
            }
            if pos == write_pos {
                break;
            }
            out.bandwidth = out.bandwidth.max(self.info[pos as usize].bandwidth);
        }
        out.tonality = max16(tonality_avg / tonality_count as f32, tonality_max - 0.2);

        let mut mpos = pos0;
        let mut vpos = pos0;
        // With enough look-ahead, against the music probability's delay of about 5 frames
        // and the activity's of about one.
        if curr_lookahead > 15 {
            mpos += 5;
            if mpos >= ds {
                mpos -= ds;
            }
            vpos += 1;
            if vpos >= ds {
                vpos -= ds;
            }
        }
        // The threshold at which switching now, rather than at any later frame in view,
        // costs least (a switch during activity costs TRANSITION_PENALTY); then whether to
        // switch at all, by the average music probability. music_prob_min is the threshold
        // for switching to music from speech, music_prob_max the other way.
        let mut prob_min = 1.0f32;
        let mut prob_max = 0.0f32;
        let vad_prob = self.info[vpos as usize].activity_probability;
        let mut prob_count = max16(0.1, vad_prob);
        let mut prob_avg = max16(0.1, vad_prob) * self.info[mpos as usize].music_prob;
        loop {
            mpos += 1;
            if mpos == ds {
                mpos = 0;
            }
            if mpos == write_pos {
                break;
            }
            vpos += 1;
            if vpos == ds {
                vpos = 0;
            }
            if vpos == write_pos {
                break;
            }
            let pos_vad = self.info[vpos as usize].activity_probability;
            prob_min = min16(
                (prob_avg - TRANSITION_PENALTY * (vad_prob - pos_vad)) / prob_count,
                prob_min,
            );
            prob_max = max16(
                (prob_avg + TRANSITION_PENALTY * (vad_prob - pos_vad)) / prob_count,
                prob_max,
            );
            prob_count += max16(0.1, pos_vad);
            prob_avg += max16(0.1, pos_vad) * self.info[mpos as usize].music_prob;
        }
        out.music_prob = prob_avg / prob_count;
        prob_min = min16(prob_avg / prob_count, prob_min);
        prob_max = max16(prob_avg / prob_count, prob_max);
        prob_min = max16(prob_min, 0.0);
        prob_max = min16(prob_max, 1.0);

        // Without enough look-ahead, the best that can be done.
        if curr_lookahead < 10 {
            let mut pmin = prob_min;
            let mut pmax = prob_max;
            pos = pos0;
            // The extremes of the past.
            for _ in 0..(self.count - 1).clamp(0, 15) {
                pos -= 1;
                if pos < 0 {
                    pos = ds - 1;
                }
                pmin = min16(pmin, self.info[pos as usize].music_prob);
                pmax = max16(pmax, self.info[pos as usize].music_prob);
            }
            // A bias against switching during activity.
            pmin = max16(0.0, pmin - 0.1 * vad_prob);
            pmax = min16(1.0, pmax + 0.1 * vad_prob);
            prob_min += (1.0 - 0.1 * curr_lookahead as f32) * (pmin - prob_min);
            prob_max += (1.0 - 0.1 * curr_lookahead as f32) * (pmax - prob_max);
        }
        out.music_prob_min = prob_min;
        out.music_prob_max = prob_max;
        out
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
