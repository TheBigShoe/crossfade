//! The surround masking analysis of libopus 1.5.2's multistream encoder
//! (`surround_analysis` in `src/opus_multistream_encoder.c`, 3-clause BSD, see the crate's
//! `COPYING`): each channel's band energies (CELT's MDCT of the pre-emphasised input),
//! spread across bands, then less what the channels on its side of the room (left, right,
//! the centre half to each) mask: a band another channel drowns out needs fewer bits. The
//! multistream encoder hands each stream its channels' masks (`OPUS_SET_ENERGY_MASK`).
#![allow(clippy::needless_range_loop)] // index loops as the reference writes them

use super::encoder::compute_band_energies;
use super::maths::log2;
use super::mdct;
use super::quant_bands::amp2_log2;
use super::tables::{MAX_LM, NB_EBANDS, OVERLAP, PREEMPH, SHORT_MDCT_SIZE};

/// The C macros, whose NaN behaviour `f32::min` and `max` do not share.
fn min16(a: f32, b: f32) -> f32 {
    if a < b { a } else { b }
}

fn max16(a: f32, b: f32) -> f32 {
    if a > b { a } else { b }
}

/// `channel_pos`: where each channel sits in the mix, 0 not mixed (the LFE), 1 left, 2 centre,
/// 3 right.
fn channel_pos(channels: usize) -> [usize; 8] {
    match channels {
        4 => [1, 3, 1, 3, 0, 0, 0, 0],
        3 | 5 | 6 => [1, 2, 3, 1, 3, 0, 0, 0],
        7 => [1, 2, 3, 1, 3, 2, 0, 0],
        8 => [1, 2, 3, 1, 3, 1, 3, 0],
        _ => [0; 8],
    }
}

/// `logSum`: about `log2(2^a + 2^b)`, by a table of the correction.
fn log_sum(a: f32, b: f32) -> f32 {
    const DIFF_TABLE: [f32; 17] = [
        0.5,
        0.292_481_3,
        0.160_964,
        0.084_962_5,
        0.043_731_4,
        0.022_197_1,
        0.011_183_9,
        0.005_613_6,
        0.002_812_3,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
    ];
    let (max, diff) = if a > b { (a, a - b) } else { (b, b - a) };
    // Inverted to catch NaNs.
    if diff.partial_cmp(&8.0) != Some(std::cmp::Ordering::Less) {
        return max;
    }
    let low = libm::floor(f64::from(2.0 * diff)) as usize;
    let frac = 2.0 * diff - low as f32;
    max + DIFF_TABLE[low] + frac * (DIFF_TABLE[low + 1] - DIFF_TABLE[low])
}

/// The state `surround_analysis` keeps per channel: the last overlap of the pre-emphasised
/// input and the pre-emphasis memory.
#[derive(Debug, Clone)]
pub(crate) struct SurroundAnalysis {
    channels: usize,
    mem: Vec<[f32; OVERLAP]>,
    preemph: Vec<f32>,
    input: Vec<f32>,
}

impl SurroundAnalysis {
    /// An analysis of `channels` (3 to 8) channels in the Vorbis order.
    pub(crate) fn new(channels: usize) -> Self {
        Self {
            channels,
            mem: vec![[0.0; OVERLAP]; channels],
            preemph: vec![0.0; channels],
            input: Vec::with_capacity(2880 + OVERLAP),
        }
    }

    #[cfg(test)]
    pub(crate) fn channels(&self) -> usize {
        self.channels
    }

    /// Forgets the stream (the multistream `OPUS_RESET_STATE`).
    pub(crate) fn reset(&mut self) {
        self.mem.fill([0.0; OVERLAP]);
        self.preemph.fill(0.0);
    }

    /// The masks of one frame of `frame_size` samples (`pcm` interleaved, full scale 1.0):
    /// for each channel, 21 band values (log2 of amplitude) into `out`, how far each band
    /// stands above what the others mask; 0 for an unmixed channel.
    pub(crate) fn analyse(&mut self, pcm: &[f32], frame_size: usize, out: &mut [f32]) {
        let ch = self.channels;
        let lm = (0..MAX_LM)
            .find(|&lm| SHORT_MDCT_SIZE << lm == frame_size)
            .unwrap_or(MAX_LM);
        let freq_size = frame_size.min(960);
        let nb_frames = frame_size / freq_size;
        let pos = channel_pos(ch);
        let mut mask_log_e = [[-28.0f32; NB_EBANDS]; 3];
        let mut freq = [0.0f32; 960];
        self.input.clear();
        self.input.resize(frame_size + OVERLAP, 0.0);
        let input = &mut self.input;
        for c in 0..ch {
            input[..OVERLAP].copy_from_slice(&self.mem[c]);
            let mut m = self.preemph[c];
            for i in 0..frame_size {
                let x = pcm[i * ch + c] * 32768.0;
                input[OVERLAP + i] = x - m;
                m = PREEMPH * x;
            }
            self.preemph[c] = m;
            let mut sum = 0.0f32;
            for &v in input.iter() {
                sum += v * v;
            }
            // NaNs, and signals absurd enough to make them further on.
            if sum.partial_cmp(&1e18) != Some(std::cmp::Ordering::Less) {
                input.fill(0.0);
                self.preemph[c] = 0.0;
            }
            let mut band_e = [0.0f32; 2 * NB_EBANDS];
            for frame in 0..nb_frames {
                let mut tmp_e = [0.0f32; 2 * NB_EBANDS];
                let at = 960 * frame;
                mdct::forward(
                    &input[at..at + freq_size + OVERLAP],
                    &mut freq[..freq_size],
                    1,
                    MAX_LM - lm,
                );
                compute_band_energies(&freq, &mut tmp_e, NB_EBANDS, 1, lm);
                // Of several frames, the loudest.
                for i in 0..NB_EBANDS {
                    band_e[i] = max16(band_e[i], tmp_e[i]);
                }
            }
            let mut log_e = [0.0f32; 2 * NB_EBANDS];
            amp2_log2(&band_e, &mut log_e, NB_EBANDS, NB_EBANDS, 1);
            let b = &mut out[NB_EBANDS * c..NB_EBANDS * (c + 1)];
            b.copy_from_slice(&log_e[..NB_EBANDS]);
            // Spreading: -6 dB a band upwards, -12 dB downwards.
            for i in 1..NB_EBANDS {
                b[i] = max16(b[i], b[i - 1] - 1.0);
            }
            for i in (0..NB_EBANDS - 1).rev() {
                b[i] = max16(b[i], b[i + 1] - 2.0);
            }
            match pos[c] {
                1 => {
                    for i in 0..NB_EBANDS {
                        mask_log_e[0][i] = log_sum(mask_log_e[0][i], b[i]);
                    }
                }
                3 => {
                    for i in 0..NB_EBANDS {
                        mask_log_e[2][i] = log_sum(mask_log_e[2][i], b[i]);
                    }
                }
                2 => {
                    for i in 0..NB_EBANDS {
                        mask_log_e[0][i] = log_sum(mask_log_e[0][i], b[i] - 0.5);
                        mask_log_e[2][i] = log_sum(mask_log_e[2][i], b[i] - 0.5);
                    }
                }
                _ => {}
            }
            self.mem[c].copy_from_slice(&input[frame_size..frame_size + OVERLAP]);
        }
        for i in 0..NB_EBANDS {
            mask_log_e[1][i] = min16(mask_log_e[0][i], mask_log_e[2][i]);
        }
        let channel_offset = 0.5 * log2(2.0 / (ch - 1) as f32);
        for m in &mut mask_log_e {
            for v in m.iter_mut() {
                *v += channel_offset;
            }
        }
        for c in 0..ch {
            let b = &mut out[NB_EBANDS * c..NB_EBANDS * (c + 1)];
            if pos[c] != 0 {
                let mask = &mask_log_e[pos[c] - 1];
                for i in 0..NB_EBANDS {
                    b[i] -= mask[i];
                }
            } else {
                b.fill(0.0);
            }
        }
    }
}

#[cfg(test)]
#[path = "surround_tests.rs"]
mod tests;
