//! Requantisation (ISO/IEC 11172-3 2.4.3.4.7): each line times 2 to the power of its band's
//! gain, in quarter steps.

use crate::scalefactors::Scalefactors;
use crate::sideinfo::Granule;
use crate::tables::PRETAB;

/// `2^(e/4)` for `e` in `GAIN_MIN..GAIN_MIN + gains.len()`.
pub(crate) const GAIN_MIN: i32 = -512;

/// Scales the first `n` lines of `xr` by their bands' gains.
pub(crate) fn apply(xr: &mut [f32; 576], g: &Granule, sf: &Scalefactors, n: usize, gains: &[f32]) {
    let bands = &g.bands;
    let n_long = usize::from(bands.n_long);
    let shift = if g.scalefac_scale { 2 } else { 1 };
    // The synthesis window is the standard's times 65536 and its output is read at 1/32768,
    // a factor of 2 that the gain takes back here (a quarter-step exponent of -4).
    let base = i32::from(g.global_gain) - 210 - 4;
    let mut line = 0;
    for k in 0..bands.count() {
        if line >= n {
            break;
        }
        let width = usize::from(bands.width[k]);
        let s = i32::from(sf.value[k]);
        let e = if k < n_long {
            let pre = if g.preflag {
                PRETAB.get(k).map_or(0, |&p| i32::from(p))
            } else {
                0
            };
            base - ((s + pre) << shift)
        } else {
            let window = (k - n_long) % 3;
            base - 8 * i32::from(g.subblock_gain[window]) - (s << shift)
        };
        let gain = gains[(e - GAIN_MIN) as usize];
        let end = (line + width).min(576);
        scale(&mut xr[line..end], gain);
        line = end;
    }
}

#[inline(always)]
fn scale(x: &mut [f32], gain: f32) {
    for v in x {
        *v *= gain;
    }
}
