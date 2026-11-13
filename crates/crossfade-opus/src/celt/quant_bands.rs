//! Band energy decoding (`celt/quant_bands.c`): coarse (Laplace, predicted across time and
//! frequency), fine and final bits, and the log to linear conversion with RFC 8251's cap.
//! Energies are log2 amplitudes, one row of [`NB_EBANDS`] per channel.

use super::ec::Ec;
use super::laplace;
use super::maths::exp2;
use super::tables::{
    BETA_COEF, BETA_INTRA, E_MEANS, E_PROB_MODEL, NB_EBANDS, PRED_COEF, SMALL_ENERGY_ICDF,
};

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
