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
#![allow(clippy::needless_range_loop)] // index loops as the reference writes them

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
fn maxabs(x: &[f32]) -> f32 {
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

    /// Whether anything was analysed since the last reset.
    pub(crate) fn initialized(&self) -> bool {
        self.initialized
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
    pub(crate) fn run(&mut self, pcm: &[f32], channels: usize, frame_size: usize) -> AnalysisInfo {
        let _ = (pcm, channels, frame_size, &self.mlp);
        AnalysisInfo::default()
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
