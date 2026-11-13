//! Spectral tools applied between dequantisation and the filterbank: M/S and intensity stereo
//! (14496-3 4.6.8), perceptual noise substitution (4.6.13), temporal noise shaping (4.6.9).

use super::ics::{INTENSITY_HCB, INTENSITY_HCB2, Ics, NOISE_HCB};
use super::tables::{TNS_MAX_BANDS_LONG, TNS_MAX_BANDS_SHORT};
use std::sync::OnceLock;

/// The CPE's M/S decision per group and band.
pub(crate) struct MsMask {
    /// 0: none, 1: per band, 2: every band.
    pub(crate) present: u8,
    pub(crate) used: [[bool; super::ics::MAX_SFB]; 8],
}

impl MsMask {
    pub(crate) fn used(&self, g: usize, b: usize) -> bool {
        match self.present {
            1 => self.used[g][b],
            2 => true,
            _ => false,
        }
    }
}

fn is_intensity(cb: u8) -> bool {
    cb == INTENSITY_HCB || cb == INTENSITY_HCB2
}

/// The windows of group `g`.
fn windows(ics: &Ics, g: usize) -> std::ops::Range<usize> {
    let first: usize = ics.info.group_len[..g].iter().sum();
    first..first + ics.info.group_len[g]
}

/// Mid/side to left/right, where used and neither channel is noise or intensity.
pub(crate) fn ms_stereo(ms: &MsMask, left: &mut Ics, right: &mut Ics) {
    if ms.present == 0 {
        return;
    }
    for g in 0..left.info.num_groups {
        for b in 0..left.info.max_sfb {
            let (cl, cr) = (left.band_cb[g][b], right.band_cb[g][b]);
            if !ms.used(g, b) || cl == NOISE_HCB || cr == NOISE_HCB || is_intensity(cr) {
                continue;
            }
            for w in windows(left, g) {
                let range = left.band(w, b);
                for (l, r) in left.spec[range.clone()]
                    .iter_mut()
                    .zip(&mut right.spec[range])
                {
                    let (m, s) = (*l, *r);
                    *l = m + s;
                    *r = m - s;
                }
            }
        }
    }
}

/// Right channel bands coded with an intensity book: the left channel scaled.
pub(crate) fn intensity_stereo(ms: &MsMask, left: &Ics, right: &mut Ics) {
    for g in 0..right.info.num_groups {
        for b in 0..right.info.max_sfb {
            let cb = right.band_cb[g][b];
            if !is_intensity(cb) {
                continue;
            }
            let mut sign = if cb == INTENSITY_HCB { 1.0 } else { -1.0 };
            if ms.present == 1 && ms.used[g][b] {
                sign = -sign;
            }
            let scale = sign * intensity_gain(right.sf[g][b]);
            for w in windows(right, g) {
                let range = right.band(w, b);
                for (r, &l) in right.spec[range.clone()].iter_mut().zip(&left.spec[range]) {
                    *r = l * scale;
                }
            }
        }
    }
}

/// 2^(-position / 4), from a table for the positions streams use.
fn intensity_gain(position: i32) -> f32 {
    const LOW: i32 = -256;
    static T: OnceLock<[f32; 512]> = OnceLock::new();
    let exact = |p: i32| libm::exp2(-0.25 * f64::from(p)) as f32;
    let table = T.get_or_init(|| std::array::from_fn(|i| exact(i as i32 + LOW)));
    match usize::try_from(position - LOW) {
        Ok(i) if i < table.len() => table[i],
        _ => exact(position),
    }
}

/// The noise generator: a 32-bit linear congruential generator (Numerical Recipes' constants),
/// seeded the same at every reset, so output is reproducible. The standard leaves the noise
/// unspecified beyond its energy.
pub(crate) struct Noise {
    state: u32,
}

impl Noise {
    pub(crate) const SEED: u32 = 0x1F2E_3D4C;

    pub(crate) fn new() -> Self {
        Self { state: Self::SEED }
    }

    pub(crate) fn reset(&mut self) {
        self.state = Self::SEED;
    }

    fn next(&mut self) -> f32 {
        self.state = self
            .state
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        self.state as i32 as f32
    }

    /// Fills `band` with noise of energy 2^(nrg / 2).
    fn fill(&mut self, band: &mut [f32], nrg: i32) {
        let mut energy = 0.0f32;
        for x in band.iter_mut() {
            *x = self.next();
            energy += *x * *x;
        }
        if energy > 0.0 {
            let scale = (libm::exp2(0.25 * f64::from(nrg)) / libm::sqrt(f64::from(energy))) as f32;
            for x in band {
                *x *= scale;
            }
        }
    }
}

/// Noise substitution in one channel. `partner` (the left channel, for a CPE's right one) with
/// M/S used in a band where both are noise means the same noise, at this channel's energy.
pub(crate) fn pns(noise: &mut Noise, ics: &mut Ics, partner: Option<(&Ics, &MsMask)>) {
    for g in 0..ics.info.num_groups {
        for b in 0..ics.info.max_sfb {
            if ics.band_cb[g][b] != NOISE_HCB {
                continue;
            }
            let nrg = ics.sf[g][b];
            for w in windows(ics, g) {
                let range = ics.band(w, b);
                match partner {
                    Some((left, ms)) if left.band_cb[g][b] == NOISE_HCB && ms.used(g, b) => {
                        let band = &mut ics.spec[range.clone()];
                        band.copy_from_slice(&left.spec[range]);
                        let energy: f32 = band.iter().map(|x| x * x).sum();
                        if energy > 0.0 {
                            let scale = (libm::exp2(0.25 * f64::from(nrg))
                                / libm::sqrt(f64::from(energy)))
                                as f32;
                            for x in band {
                                *x *= scale;
                            }
                        }
                    }
                    _ => noise.fill(&mut ics.spec[range], nrg),
                }
            }
        }
    }
}

/// Temporal noise shaping: the all-pole filters over each window's spectrum.
pub(crate) fn tns(ics: &mut Ics, sampling_index: usize) {
    if !ics.tns.present {
        return;
    }
    let short = ics.info.is_short();
    let max_bands = usize::from(if short {
        TNS_MAX_BANDS_SHORT[sampling_index]
    } else {
        TNS_MAX_BANDS_LONG[sampling_index]
    });
    let num_swb = ics.info.swb.len() - 1;
    let limit = max_bands.min(ics.info.max_sfb);
    for w in 0..ics.info.num_windows {
        let base = if short { w * 128 } else { 0 };
        let mut top = num_swb;
        for f in 0..ics.tns.n_filt[w] {
            let filt = ics.tns.filt[w][f];
            let bottom = top.saturating_sub(filt.length);
            let (lo, hi) = (bottom.min(limit), top.min(limit));
            top = bottom;
            if filt.order == 0 {
                continue;
            }
            let start = base + usize::from(ics.info.swb[lo]);
            let end = base + usize::from(ics.info.swb[hi]);
            if end <= start {
                continue;
            }
            let lpc = &filt.lpc[..filt.order];
            let band = &mut ics.spec[start..end];
            let mut state = [0f32; 12];
            let mut run = |x: &mut f32| {
                let mut y = *x;
                for (a, s) in lpc.iter().zip(&state) {
                    y -= a * s;
                }
                state.copy_within(0..11, 1);
                state[0] = y;
                *x = y;
            };
            if filt.direction {
                band.iter_mut().rev().for_each(&mut run);
            } else {
                band.iter_mut().for_each(&mut run);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intensity_gains_are_the_exact_power_inside_and_outside_the_table() {
        for p in -600..600 {
            let want = libm::exp2(-0.25 * f64::from(p)) as f32;
            assert_eq!(intensity_gain(p).to_bits(), want.to_bits(), "{p}");
        }
    }
}
