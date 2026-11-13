//! Bit allocation (`celt/rate.c`, `rate.h`): which bands get how many bits, in exact integer
//! arithmetic, because it decides what is read next.

use super::ec::Ec;
use super::tables::{
    BAND_ALLOCATION, CACHE_BITS, CACHE_CAPS, CACHE_INDEX, EBANDS, LOG_N, LOG2_FRAC_TABLE,
    NB_ALLOC_VECTORS, NB_EBANDS,
};

/// Bits are counted in 1/8 bit (`BITRES`).
pub(super) const BITRES: i32 = 3;
const MAX_FINE_BITS: i32 = 8;
const FINE_OFFSET: i32 = 21;
const LOG_MAX_PSEUDO: usize = 6;
const ALLOC_STEPS: i32 = 6;

fn band_width(j: usize) -> i32 {
    i32::from(EBANDS[j + 1] - EBANDS[j])
}

/// `get_pulses`: the pulse count of pseudo-pulse index `i`.
pub(super) fn get_pulses(i: i32) -> i32 {
    if i < 8 {
        i
    } else {
        (8 + (i & 7)) << ((i >> 3) - 1)
    }
}

/// The pulse cache of a band at `lm` (-1 to 3): entry 0 is the largest pseudo-pulse index,
/// entry `k` the bits (in 1/8 bit, minus one) that `k` pseudo-pulses cost.
fn cache(band: usize, lm: i32) -> &'static [u8] {
    let row = (lm + 1).clamp(0, 4) as usize;
    match usize::try_from(CACHE_INDEX[row * NB_EBANDS + band]) {
        Ok(at) => &CACHE_BITS[at..],
        Err(_) => &[0],
    }
}

/// The largest pseudo-pulse cost of a band, plus 12: `quant_band` splits above it.
pub(super) fn split_threshold(band: usize, lm: i32) -> i32 {
    let c = cache(band, lm);
    i32::from(c[usize::from(c[0])]) + 12
}

/// `bits2pulses`: the pseudo-pulse index whose cost is nearest `bits`.
pub(super) fn bits2pulses(band: usize, lm: i32, bits: i32) -> i32 {
    let cache = cache(band, lm);
    let mut lo = 0usize;
    let mut hi = usize::from(cache[0]);
    let bits = bits - 1;
    for _ in 0..LOG_MAX_PSEUDO {
        let mid = (lo + hi + 1) >> 1;
        if i32::from(cache[mid]) >= bits {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    let below = if lo == 0 { -1 } else { i32::from(cache[lo]) };
    if bits - below <= i32::from(cache[hi]) - bits {
        lo as i32
    } else {
        hi as i32
    }
}

/// `pulses2bits`.
pub(super) fn pulses2bits(band: usize, lm: i32, pulses: i32) -> i32 {
    if pulses == 0 {
        0
    } else {
        let cache = cache(band, lm);
        cache.get(pulses as usize).map_or(0, |&c| i32::from(c) + 1)
    }
}

/// `init_caps`: the most bits each band can use.
pub(super) fn init_caps(lm: usize, c: usize) -> [i32; NB_EBANDS] {
    let mut cap = [0; NB_EBANDS];
    for (i, cap) in cap.iter_mut().enumerate() {
        let n = band_width(i) << lm;
        let caps = i32::from(CACHE_CAPS[NB_EBANDS * (2 * lm + c - 1) + i]);
        *cap = (caps + 64) * c as i32 * n >> 2;
    }
    cap
}

/// What `compute_allocation` decides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Allocation {
    /// One past the last band given PVQ bits (`codedBands`).
    pub(super) coded_bands: usize,
    pub(super) intensity: usize,
    pub(super) dual_stereo: bool,
    /// Bits over the caps, for `quant_all_bands` to rebalance.
    pub(super) balance: i32,
    /// PVQ bits per band, in 1/8 bit.
    pub(super) pulses: [i32; NB_EBANDS],
    /// Fine energy bits per band and channel.
    pub(super) fine_quant: [i32; NB_EBANDS],
    pub(super) fine_priority: [i32; NB_EBANDS],
}

/// `compute_allocation` (decoding): splits `total` (1/8 bit) over the bands `start..end`,
/// reading the skip, intensity and dual-stereo decisions.
#[allow(clippy::too_many_arguments)]
pub(super) fn compute_allocation(
    start: usize,
    end: usize,
    offsets: &[i32; NB_EBANDS],
    cap: &[i32; NB_EBANDS],
    alloc_trim: i32,
    total: i32,
    c: usize,
    lm: usize,
    ec: &mut Ec<'_, '_>,
) -> Allocation {
    let cc = c as i32;
    let lmi = lm as i32;
    let mut total = total.max(0);
    let mut skip_start = start;
    let skip_rsv = if total >= 1 << BITRES { 1 << BITRES } else { 0 };
    total -= skip_rsv;
    let mut intensity_rsv = 0;
    let mut dual_stereo_rsv = 0;
    if c == 2 {
        intensity_rsv = i32::from(LOG2_FRAC_TABLE[end - start]);
        if intensity_rsv > total {
            intensity_rsv = 0;
        } else {
            total -= intensity_rsv;
            dual_stereo_rsv = if total >= 1 << BITRES { 1 << BITRES } else { 0 };
            total -= dual_stereo_rsv;
        }
    }
    let mut bits1 = [0; NB_EBANDS];
    let mut bits2 = [0; NB_EBANDS];
    let mut thresh = [0; NB_EBANDS];
    let mut trim_offset = [0; NB_EBANDS];
    for j in start..end {
        let n = band_width(j);
        thresh[j] = (cc << BITRES).max((3 * n << lmi << BITRES) >> 4);
        trim_offset[j] =
            cc * n * (alloc_trim - 5 - lmi) * (end - j - 1) as i32 * (1 << (lmi + BITRES)) >> 6;
        if n << lmi == 1 {
            trim_offset[j] -= cc << BITRES;
        }
    }
    let alloc = |row: usize, j: usize| -> i32 {
        cc * band_width(j) * i32::from(BAND_ALLOCATION[row * NB_EBANDS + j]) << lmi >> 2
    };
    let mut lo = 1i32;
    let mut hi = NB_ALLOC_VECTORS as i32 - 1;
    loop {
        let mut done = false;
        let mut psum = 0;
        let mid = (lo + hi) >> 1;
        for j in (start..end).rev() {
            let mut bitsj = alloc(mid as usize, j);
            if bitsj > 0 {
                bitsj = (bitsj + trim_offset[j]).max(0);
            }
            bitsj += offsets[j];
            if bitsj >= thresh[j] || done {
                done = true;
                psum += bitsj.min(cap[j]);
            } else if bitsj >= cc << BITRES {
                psum += cc << BITRES;
            }
        }
        if psum > total {
            hi = mid - 1;
        } else {
            lo = mid + 1;
        }
        if lo > hi {
            break;
        }
    }
    hi = lo;
    lo -= 1;
    for j in start..end {
        let mut bits1j = alloc(lo as usize, j);
        let mut bits2j = if hi as usize >= NB_ALLOC_VECTORS {
            cap[j]
        } else {
            alloc(hi as usize, j)
        };
        if bits1j > 0 {
            bits1j = (bits1j + trim_offset[j]).max(0);
        }
        if bits2j > 0 {
            bits2j = (bits2j + trim_offset[j]).max(0);
        }
        if lo > 0 {
            bits1j += offsets[j];
        }
        bits2j += offsets[j];
        if offsets[j] > 0 {
            skip_start = j;
        }
        bits1[j] = bits1j;
        bits2[j] = (bits2j - bits1j).max(0);
    }
    interp_bits2pulses(Interp {
        start,
        end,
        skip_start,
        bits1: &bits1,
        bits2: &bits2,
        thresh: &thresh,
        cap,
        total,
        skip_rsv,
        intensity_rsv,
        dual_stereo_rsv,
        c,
        lm,
        ec,
    })
}

struct Interp<'a, 'e, 'b, 'c> {
    start: usize,
    end: usize,
    skip_start: usize,
    bits1: &'a [i32; NB_EBANDS],
    bits2: &'a [i32; NB_EBANDS],
    thresh: &'a [i32; NB_EBANDS],
    cap: &'a [i32; NB_EBANDS],
    total: i32,
    skip_rsv: i32,
    intensity_rsv: i32,
    dual_stereo_rsv: i32,
    c: usize,
    lm: usize,
    ec: &'e mut Ec<'b, 'c>,
}

fn interp_bits2pulses(a: Interp<'_, '_, '_, '_>) -> Allocation {
    let Interp {
        start,
        end,
        skip_start,
        bits1,
        bits2,
        thresh,
        cap,
        mut total,
        skip_rsv,
        mut intensity_rsv,
        mut dual_stereo_rsv,
        c,
        lm,
        ec,
    } = a;
    let cc = c as i32;
    let alloc_floor = cc << BITRES;
    let stereo = i32::from(c > 1);
    let log_m = (lm as i32) << BITRES;
    let eb = |j: usize| i32::from(EBANDS[j]);

    let mut lo = 0;
    let mut hi = 1 << ALLOC_STEPS;
    for _ in 0..ALLOC_STEPS {
        let mid = (lo + hi) >> 1;
        let mut psum = 0;
        let mut done = false;
        for j in (start..end).rev() {
            let tmp = bits1[j] + (mid * bits2[j] >> ALLOC_STEPS);
            if tmp >= thresh[j] || done {
                done = true;
                psum += tmp.min(cap[j]);
            } else if tmp >= alloc_floor {
                psum += alloc_floor;
            }
        }
        if psum > total {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    let mut psum = 0;
    let mut done = false;
    let mut bits = [0; NB_EBANDS];
    for j in (start..end).rev() {
        let mut tmp = bits1[j] + (lo * bits2[j] >> ALLOC_STEPS);
        if tmp < thresh[j] && !done {
            tmp = if tmp >= alloc_floor { alloc_floor } else { 0 };
        } else {
            done = true;
        }
        tmp = tmp.min(cap[j]);
        bits[j] = tmp;
        psum += tmp;
    }

    let mut coded_bands = end;
    loop {
        let j = coded_bands - 1;
        if j <= skip_start {
            total += skip_rsv;
            break;
        }
        let mut left = total - psum;
        let percoeff = left / (eb(coded_bands) - eb(start));
        left -= (eb(coded_bands) - eb(start)) * percoeff;
        let rem = (left - (eb(j) - eb(start))).max(0);
        let band_width = eb(coded_bands) - eb(j);
        let mut band_bits = bits[j] + percoeff * band_width + rem;
        if band_bits >= thresh[j].max(alloc_floor + (1 << BITRES)) {
            if ec.bit_logp(1) {
                break;
            }
            psum += 1 << BITRES;
            band_bits -= 1 << BITRES;
        }
        psum -= bits[j] + intensity_rsv;
        if intensity_rsv > 0 {
            intensity_rsv = i32::from(LOG2_FRAC_TABLE[j - start]);
        }
        psum += intensity_rsv;
        if band_bits >= alloc_floor {
            psum += alloc_floor;
            bits[j] = alloc_floor;
        } else {
            bits[j] = 0;
        }
        coded_bands -= 1;
    }

    let mut intensity = 0;
    if intensity_rsv > 0 {
        intensity = start + ec.uint((coded_bands + 1 - start) as u32) as usize;
    }
    if intensity <= start {
        total += dual_stereo_rsv;
        dual_stereo_rsv = 0;
    }
    let dual_stereo = dual_stereo_rsv > 0 && ec.bit_logp(1);

    let mut left = total - psum;
    let percoeff = left / (eb(coded_bands) - eb(start));
    left -= (eb(coded_bands) - eb(start)) * percoeff;
    for j in start..coded_bands {
        bits[j] += percoeff * band_width(j);
    }
    for j in start..coded_bands {
        let tmp = left.min(band_width(j));
        bits[j] += tmp;
        left -= tmp;
    }

    let mut ebits = [0; NB_EBANDS];
    let mut fine_priority = [0; NB_EBANDS];
    let mut balance = 0;
    for j in start..coded_bands {
        let n0 = band_width(j);
        let n = n0 << lm;
        bits[j] += balance;
        let mut excess;
        if n > 1 {
            excess = (bits[j] - cap[j]).max(0);
            bits[j] -= excess;
            let den = cc * n + i32::from(c == 2 && n > 2 && !dual_stereo && j < intensity);
            let nclogn = den * (i32::from(LOG_N[j]) + log_m);
            let mut offset = (nclogn >> 1) - den * FINE_OFFSET;
            if n == 2 {
                offset += den << BITRES >> 2;
            }
            if bits[j] + offset < den * 2 << BITRES {
                offset += nclogn >> 2;
            } else if bits[j] + offset < den * 3 << BITRES {
                offset += nclogn >> 3;
            }
            ebits[j] = ((bits[j] + offset + (den << (BITRES - 1))) / (den << BITRES)).max(0);
            if cc * ebits[j] > bits[j] >> BITRES {
                ebits[j] = bits[j] >> stereo >> BITRES;
            }
            ebits[j] = ebits[j].min(MAX_FINE_BITS);
            fine_priority[j] = i32::from(ebits[j] * (den << BITRES) >= bits[j] + offset);
            bits[j] -= cc * ebits[j] << BITRES;
        } else {
            excess = (bits[j] - (cc << BITRES)).max(0);
            bits[j] -= excess;
            ebits[j] = 0;
            fine_priority[j] = 1;
        }
        if excess > 0 {
            let extra_fine = (excess >> (stereo + BITRES)).min(MAX_FINE_BITS - ebits[j]);
            ebits[j] += extra_fine;
            let extra_bits = extra_fine * cc << BITRES;
            fine_priority[j] = i32::from(extra_bits >= excess - balance);
            excess -= extra_bits;
        }
        balance = excess;
    }
    for j in coded_bands..end {
        ebits[j] = bits[j] >> stereo >> BITRES;
        bits[j] = 0;
        fine_priority[j] = i32::from(ebits[j] < 1);
    }
    Allocation {
        coded_bands,
        intensity,
        dual_stereo,
        balance,
        pulses: bits,
        fine_quant: ebits,
        fine_priority,
    }
}

#[cfg(test)]
#[path = "rate_tests.rs"]
mod tests;
