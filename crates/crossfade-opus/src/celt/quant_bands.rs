//! Band energies (`celt/quant_bands.c`), decoded and encoded: coarse (Laplace, predicted
//! across time and frequency; the encoder's two-pass intra/inter choice), fine and final bits,
//! and the log to linear conversions (with RFC 8251's cap). Energies are log2 amplitudes, one
//! row of [`NB_EBANDS`] per channel.
#![allow(clippy::needless_range_loop)] // index loops as the reference writes them

use super::ec::Ec;
use super::laplace;
use super::maths::{exp2, log2};
use super::tables::{
    BETA_COEF, BETA_INTRA, E_MEANS, E_PROB_MODEL, NB_EBANDS, PRED_COEF, SMALL_ENERGY_ICDF,
};
use crate::range_enc::RangeEncoder;

const MAX_FINE_BITS: i32 = 8;

/// `unquant_coarse_energy`.
pub(super) fn unquant_coarse_energy(
    old: &mut [f32; 2 * NB_EBANDS],
    start: usize,
    end: usize,
    intra: bool,
    ec: &mut Ec<'_, '_>,
    c: usize,
    lm: usize,
) {
    let prob_model = &E_PROB_MODEL[lm][usize::from(intra)];
    let mut prev = [0.0f32; 2];
    let (coef, beta) = if intra {
        (0.0, BETA_INTRA)
    } else {
        (PRED_COEF[lm], BETA_COEF[lm])
    };
    let budget = ec.storage() * 8;
    for i in start..end {
        for (ch, prev) in prev.iter_mut().enumerate().take(c) {
            let tell = ec.tell();
            let left = budget.wrapping_sub(tell);
            let qi: i32 = if left >= 15 {
                let pi = 2 * i.min(20);
                laplace::decode(
                    ec,
                    u32::from(prob_model[pi]) << 7,
                    u32::from(prob_model[pi + 1]) << 6,
                )
            } else if left >= 2 {
                let qi = ec.icdf(&SMALL_ENERGY_ICDF, 2) as i32;
                (qi >> 1) ^ -(qi & 1)
            } else if left >= 1 {
                -i32::from(ec.bit_logp(1))
            } else {
                -1
            };
            let q = qi as f32;
            let e = &mut old[i + ch * NB_EBANDS];
            *e = e.max(-9.0);
            let tmp = coef * *e + *prev + q;
            *e = tmp;
            *prev = *prev + q - beta * q;
        }
    }
}

/// `unquant_fine_energy`.
pub(super) fn unquant_fine_energy(
    old: &mut [f32; 2 * NB_EBANDS],
    start: usize,
    end: usize,
    fine_quant: &[i32; NB_EBANDS],
    ec: &mut Ec<'_, '_>,
    c: usize,
) {
    for i in start..end {
        let bits = fine_quant[i];
        if bits <= 0 {
            continue;
        }
        let bits = bits.min(MAX_FINE_BITS);
        for ch in 0..c {
            let q2 = ec.bits(bits as u32);
            let offset = (q2 as f32 + 0.5) * (1 << (14 - bits)) as f32 * (1.0 / 16384.0) - 0.5;
            old[i + ch * NB_EBANDS] += offset;
        }
    }
}

/// `unquant_energy_finalise`: one more bit per band and channel while `bits_left` lasts,
/// bands of priority 0 first.
#[allow(clippy::too_many_arguments)]
pub(super) fn unquant_energy_finalise(
    old: &mut [f32; 2 * NB_EBANDS],
    start: usize,
    end: usize,
    fine_quant: &[i32; NB_EBANDS],
    fine_priority: &[i32; NB_EBANDS],
    mut bits_left: i32,
    ec: &mut Ec<'_, '_>,
    c: usize,
) {
    for prio in 0..2 {
        let mut i = start;
        while i < end && bits_left >= c as i32 {
            if fine_quant[i] >= MAX_FINE_BITS || fine_priority[i] != prio {
                i += 1;
                continue;
            }
            for ch in 0..c {
                let q2 = ec.bits(1);
                let offset =
                    (q2 as f32 - 0.5) * (1 << (14 - fine_quant[i] - 1)) as f32 * (1.0 / 16384.0);
                old[i + ch * NB_EBANDS] += offset;
                bits_left -= 1;
            }
            i += 1;
        }
    }
}

/// What [`quant_coarse_energy`] needs besides the energies.
pub(super) struct Coarse {
    pub(super) start: usize,
    pub(super) end: usize,
    /// One past the last band of real energy (the mode's `effEBands` or `end`).
    pub(super) eff_end: usize,
    /// The frame's bits (`total_bits`).
    pub(super) budget: i32,
    pub(super) c: usize,
    pub(super) lm: usize,
    pub(super) nb_available_bytes: i32,
    pub(super) force_intra: bool,
    /// Try intra and inter and keep the better (complexity 4 and up).
    pub(super) two_pass: bool,
    /// The LFE: energy decays at most 3 dB a frame and never rises past band 1.
    pub(super) lfe: bool,
}

/// `quant_coarse_energy_impl`: codes the intra flag and the deltas; returns the badness
/// (how far the deltas were clamped).
#[allow(clippy::too_many_arguments)]
fn quant_coarse_impl(
    p: &Coarse,
    e_bands: &[f32; 2 * NB_EBANDS],
    old: &mut [f32; 2 * NB_EBANDS],
    error: &mut [f32; 2 * NB_EBANDS],
    enc: &mut RangeEncoder,
    intra: bool,
    max_decay: f32,
) -> i32 {
    let budget = p.budget;
    let mut badness = 0;
    let mut prev = [0.0f32; 2];
    if enc.tell() + 3 <= budget {
        enc.enc_bit_logp(intra, 3);
    }
    let (coef, beta) = if intra {
        (0.0, BETA_INTRA)
    } else {
        (PRED_COEF[p.lm], BETA_COEF[p.lm])
    };
    let prob_model = &E_PROB_MODEL[p.lm][usize::from(intra)];
    for i in p.start..p.end {
        for (ch, prev) in prev.iter_mut().enumerate().take(p.c) {
            let k = i + ch * NB_EBANDS;
            let x = e_bands[k];
            let old_e = old[k].max(-9.0);
            let f = x - coef * old_e - *prev;
            // Rounding to nearest matters here.
            let mut qi = libm::floorf(0.5 + f) as i32;
            let decay_bound = old[k].max(-28.0) - max_decay;
            // Keep the energy from falling too fast (bands of one bin, say).
            if qi < 0 && x < decay_bound {
                qi += (decay_bound - x) as i32;
                qi = qi.min(0);
            }
            let qi0 = qi;
            let tell = enc.tell();
            let bits_left = budget - tell - 3 * p.c as i32 * (p.end - i) as i32;
            if i != p.start && bits_left < 30 {
                if bits_left < 24 {
                    qi = qi.min(1);
                }
                if bits_left < 16 {
                    qi = qi.max(-1);
                }
            }
            if p.lfe && i >= 2 {
                qi = qi.min(0);
            }
            if budget - tell >= 15 {
                let pi = 2 * i.min(20);
                laplace::encode(
                    enc,
                    &mut qi,
                    u32::from(prob_model[pi]) << 7,
                    u32::from(prob_model[pi + 1]) << 6,
                );
            } else if budget - tell >= 2 {
                qi = qi.clamp(-1, 1);
                enc.enc_icdf(
                    ((2 * qi) ^ -i32::from(qi < 0)) as usize,
                    &SMALL_ENERGY_ICDF,
                    2,
                );
            } else if budget - tell >= 1 {
                qi = qi.min(0);
                enc.enc_bit_logp(qi != 0, 1);
            } else {
                qi = -1;
            }
            error[k] = f - qi as f32;
            badness += (qi0 - qi).abs();
            let q = qi as f32;
            old[k] = coef * old_e + *prev + q;
            *prev = *prev + q - beta * q;
        }
    }
    if p.lfe { 0 } else { badness }
}

/// `quant_coarse_energy`: codes `e_bands` (log2 amplitudes) against `old`, which becomes the
/// quantised energies; `error` gets what is left for the fine bits. Intra or inter as the
/// two-pass search (or `delayed_intra`, the running loss distortion) decides.
pub(super) fn quant_coarse_energy(
    p: &Coarse,
    e_bands: &[f32; 2 * NB_EBANDS],
    old: &mut [f32; 2 * NB_EBANDS],
    error: &mut [f32; 2 * NB_EBANDS],
    enc: &mut RangeEncoder,
    delayed_intra: &mut f32,
) {
    let (start, end, c) = (p.start, p.end, p.c);
    let mut two_pass = p.two_pass;
    let mut intra = p.force_intra
        || (!two_pass
            && *delayed_intra > (2 * c * (end - start)) as f32
            && p.nb_available_bytes > ((end - start) * c) as i32);
    // `intra_bias` is 0 without packet loss.
    let mut new_distortion = 0.0f32;
    for ch in 0..c {
        for i in start..p.eff_end {
            let d = e_bands[i + ch * NB_EBANDS] - old[i + ch * NB_EBANDS];
            new_distortion += d * d;
        }
    }
    let new_distortion = new_distortion.min(200.0);
    if enc.tell() + 3 > p.budget {
        two_pass = false;
        intra = false;
    }
    let mut max_decay = 16.0f32;
    if end - start > 10 {
        max_decay = max_decay.min(0.125 * p.nb_available_bytes as f32);
    }
    if p.lfe {
        max_decay = 3.0;
    }
    let start_state = enc.clone();
    let mut old_intra = *old;
    let mut error_intra = [0.0f32; 2 * NB_EBANDS];
    let mut badness1 = 0;
    if two_pass || intra {
        badness1 = quant_coarse_impl(
            p,
            e_bands,
            &mut old_intra,
            &mut error_intra,
            enc,
            true,
            max_decay,
        );
    }
    if !intra {
        let tell_intra = enc.tell_frac() as i32;
        let intra_state = std::mem::replace(enc, start_state);
        let badness2 = quant_coarse_impl(p, e_bands, old, error, enc, false, max_decay);
        if two_pass
            && (badness1 < badness2
                || (badness1 == badness2 && enc.tell_frac() as i32 > tell_intra))
        {
            *enc = intra_state;
            *old = old_intra;
            *error = error_intra;
            intra = true;
        }
    } else {
        *old = old_intra;
        *error = error_intra;
    }
    *delayed_intra = if intra {
        new_distortion
    } else {
        (PRED_COEF[p.lm] * PRED_COEF[p.lm]) * *delayed_intra + new_distortion
    };
}

/// `quant_fine_energy`: `fine_quant[i]` more bits of each band's energy, from `error`.
pub(super) fn quant_fine_energy(
    old: &mut [f32; 2 * NB_EBANDS],
    error: &mut [f32; 2 * NB_EBANDS],
    start: usize,
    end: usize,
    fine_quant: &[i32; NB_EBANDS],
    enc: &mut RangeEncoder,
    c: usize,
) {
    for i in start..end {
        let bits = fine_quant[i];
        if bits <= 0 {
            continue;
        }
        let bits = bits.min(MAX_FINE_BITS);
        let frac = 1 << bits;
        for ch in 0..c {
            let k = i + ch * NB_EBANDS;
            let q2 = (libm::floorf((error[k] + 0.5) * frac as f32) as i32).clamp(0, frac - 1);
            enc.enc_bits(q2 as u32, bits as u32);
            let offset = (q2 as f32 + 0.5) * (1 << (14 - bits)) as f32 * (1.0 / 16384.0) - 0.5;
            old[k] += offset;
            error[k] -= offset;
        }
    }
}

/// `quant_energy_finalise`: one more bit per band and channel while `bits_left` lasts,
/// bands of priority 0 first.
#[allow(clippy::too_many_arguments)]
pub(super) fn quant_energy_finalise(
    old: &mut [f32; 2 * NB_EBANDS],
    error: &mut [f32; 2 * NB_EBANDS],
    start: usize,
    end: usize,
    fine_quant: &[i32; NB_EBANDS],
    fine_priority: &[i32; NB_EBANDS],
    mut bits_left: i32,
    enc: &mut RangeEncoder,
    c: usize,
) {
    for prio in 0..2 {
        let mut i = start;
        while i < end && bits_left >= c as i32 {
            if fine_quant[i] >= MAX_FINE_BITS || fine_priority[i] != prio {
                i += 1;
                continue;
            }
            for ch in 0..c {
                let k = i + ch * NB_EBANDS;
                let q2 = u32::from(error[k] >= 0.0);
                enc.enc_bits(q2, 1);
                let offset =
                    (q2 as f32 - 0.5) * (1 << (14 - fine_quant[i] - 1)) as f32 * (1.0 / 16384.0);
                old[k] += offset;
                error[k] -= offset;
                bits_left -= 1;
            }
            i += 1;
        }
    }
}

/// `amp2Log2`: band amplitudes to log2 energies relative to the band means; -14 for the
/// bands from `eff_end` to `end`.
pub(super) fn amp2_log2(
    band_e: &[f32; 2 * NB_EBANDS],
    log_e: &mut [f32; 2 * NB_EBANDS],
    eff_end: usize,
    end: usize,
    c: usize,
) {
    for ch in 0..c {
        let row = ch * NB_EBANDS;
        for i in 0..eff_end {
            log_e[row + i] = log2(band_e[row + i]) - E_MEANS[i];
        }
        for v in &mut log_e[row + eff_end..row + end] {
            *v = -14.0;
        }
    }
}

/// `log2Amp` with RFC 8251 section 8: the log energy is capped at 32 before `celt_exp2`, so
/// the amplitude stays finite.
pub(super) fn log2_amp(
    band_e: &mut [f32; 2 * NB_EBANDS],
    old: &[f32; 2 * NB_EBANDS],
    start: usize,
    end: usize,
    c: usize,
) {
    for ch in 0..c {
        let row = ch * NB_EBANDS;
        for i in 0..NB_EBANDS {
            band_e[row + i] = if i >= start && i < end {
                let lg = (old[row + i] + E_MEANS[i]).min(32.0);
                exp2(lg)
            } else {
                0.0
            };
        }
    }
}

#[cfg(test)]
#[path = "quant_bands_tests.rs"]
mod tests;
