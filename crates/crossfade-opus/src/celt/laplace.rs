//! The Laplace-like distribution of the coarse energy deltas (`celt/laplace.c`): unsigned
//! arithmetic as the reference's, wrapping where C's would.

use super::ec::Ec;

/// The minimum probability of a delta, out of 32768 (`LAPLACE_MINP`).
const MINP: u32 = 1;
/// Deltas guaranteed representable in each direction (`LAPLACE_NMIN`).
const NMIN: u32 = 16;

/// `ec_laplace_get_freq1`: the frequency of +1 (and of -1) given that of 0.
fn freq1(fs0: u32, decay: u32) -> u32 {
    let ft = (32768 - MINP * (2 * NMIN)).wrapping_sub(fs0);
    ft.wrapping_mul(16384u32.wrapping_sub(decay)) >> 15
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
