//! The Laplace-like distribution of the coarse energy deltas (`celt/laplace.c`): unsigned
//! arithmetic as the reference's, wrapping where C's would.

use super::ec::Ec;
use crate::range_enc::RangeEncoder;

/// The minimum probability of a delta, out of 32768 (`LAPLACE_MINP`).
const MINP: u32 = 1;
/// Deltas guaranteed representable in each direction (`LAPLACE_NMIN`).
const NMIN: u32 = 16;

/// `ec_laplace_get_freq1`: the frequency of +1 (and of -1) given that of 0.
fn freq1(fs0: u32, decay: u32) -> u32 {
    let ft = (32768 - MINP * (2 * NMIN)).wrapping_sub(fs0);
    ft.wrapping_mul(16384u32.wrapping_sub(decay)) >> 15
}

/// `ec_laplace_encode`: codes `value`, clamped to what the tail can represent (the clamped
/// value is written back).
pub(super) fn encode(enc: &mut RangeEncoder, value: &mut i32, mut fs: u32, decay: u32) {
    let mut fl = 0u32;
    let val = *value;
    if val != 0 {
        let s: i32 = -i32::from(val < 0);
        let v = (val + s) ^ s;
        fl = fs;
        fs = freq1(fs, decay);
        let mut i = 1;
        while fs > 0 && i < v {
            fs *= 2;
            fl += fs + 2 * MINP;
            fs = (fs * decay) >> 15;
            i += 1;
        }
        if fs == 0 {
            let ndi_max = (32768 - fl + MINP - 1) as i32;
            let ndi_max = (ndi_max - s) >> 1;
            let di = (v - i).min(ndi_max - 1);
            fl = (fl as i32 + (2 * di + 1 + s) * MINP as i32) as u32;
            fs = MINP.min(32768 - fl);
            *value = (i + di + s) ^ s;
        } else {
            fs += MINP;
            fl += fs & !(s as u32);
        }
    }
    enc.encode_bin(fl, fl + fs, 15);
}

/// `ec_laplace_decode`: a delta whose 0 has frequency `fs` (of 32768) and whose tail falls
/// by `decay` (Q14) per step.
pub(super) fn decode(ec: &mut Ec<'_, '_>, mut fs: u32, decay: u32) -> i32 {
    let mut val: i32 = 0;
    let fm = ec.decode_bin(15);
    let mut fl = 0u32;
    if fm >= fs {
        val += 1;
        fl = fs;
        fs = freq1(fs, decay).wrapping_add(MINP);
        while fs > MINP && fm >= fl.wrapping_add(fs.wrapping_mul(2)) {
            fs = fs.wrapping_mul(2);
            fl = fl.wrapping_add(fs);
            fs = (fs.wrapping_sub(2 * MINP)).wrapping_mul(decay) >> 15;
            fs = fs.wrapping_add(MINP);
            val += 1;
        }
        if fs <= MINP {
            let di = fm.wrapping_sub(fl) >> 1;
            val = val.wrapping_add(di as i32);
            fl = fl.wrapping_add(2 * di * MINP);
        }
        if fm < fl.wrapping_add(fs) {
            val = -val;
        } else {
            fl = fl.wrapping_add(fs);
        }
    }
    ec.update(fl, fl.wrapping_add(fs).min(32768), 32768);
    val
}

#[cfg(test)]
#[path = "laplace_tests.rs"]
pub(super) mod tests;
