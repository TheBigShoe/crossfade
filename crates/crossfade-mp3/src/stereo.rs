//! Joint stereo: mid/side (ISO/IEC 11172-3 2.4.3.4.9.2) and intensity stereo in its MPEG-1
//! (2.4.3.4.9.3) and MPEG-2 (13818-3 2.4.3.2) forms, on the requantised spectra before short
//! blocks are reordered.
//!
//! Intensity stereo codes the bands above the right channel's last non-zero line (for short
//! blocks, per window) as the left channel's signal panned by the right channel's scalefactor,
//! the "intensity position". A position that is illegal (7 in MPEG-1, the largest value its
//! width can hold in MPEG-2) means the band is coded as mid/side or left/right instead. The last
//! band, which has no scalefactor of its own, takes the position of the band below it when that
//! band is intensity coded, and otherwise the middle (position 3 in MPEG-1, 0 in MPEG-2): the
//! reading that the ISO conformance streams `l3_20245`-`l3_20248` and `l3_25208` confirm.

use crate::scalefactors::Scalefactors;
use crate::sideinfo::Granule;

const SQRT_HALF: f32 = core::f32::consts::FRAC_1_SQRT_2;

/// Mid/side over lines `from..to`.
#[inline(always)]
pub(crate) fn mid_side(left: &mut [f32], right: &mut [f32]) {
    for (l, r) in left.iter_mut().zip(right.iter_mut()) {
        let (m, s) = (*l, *r);
        *l = (m + s) * SQRT_HALF;
        *r = (m - s) * SQRT_HALF;
    }
}

/// Panning gains for intensity stereo.
pub(crate) struct Pan {
    /// MPEG-1 positions 0-6: (left, right) = (tan/(1+tan), 1/(1+tan)) of `pos * pi/12`.
    pub(crate) mpeg1: [(f32, f32); 7],
    /// MPEG-2: `2^(-k/4)` for k in 0..64 (positions times the intensity scale).
    pub(crate) quarter: [f32; 64],
}

impl Pan {
    pub(crate) fn new() -> Self {
        let mut mpeg1 = [(0.0, 0.0); 7];
        for (p, m) in mpeg1.iter_mut().enumerate() {
            *m = if p == 6 {
                (1.0, 0.0)
            } else {
                let t = libm::tan(p as f64 * core::f64::consts::PI / 12.0);
                ((t / (1.0 + t)) as f32, (1.0 / (1.0 + t)) as f32)
            };
        }
        let mut quarter = [0.0; 64];
        for (k, q) in quarter.iter_mut().enumerate() {
            *q = libm::exp2(-(k as f64) / 4.0) as f32;
        }
        Self { mpeg1, quarter }
    }
}

/// One band entry: where its lines start, how many, its window (short) or none (long), and
/// its band number within its kind.
struct Entry {
    start: usize,
    width: usize,
    window: Option<usize>,
    band: usize,
}

fn entries(g: &Granule) -> ([Entry; 40], usize) {
    let bands = &g.bands;
    let n_long = usize::from(bands.n_long);
    let first_short = if g.mixed { 3 } else { 0 };
    let mut out: [Entry; 40] = core::array::from_fn(|_| Entry {
        start: 576,
        width: 0,
        window: None,
        band: 0,
    });
    let mut start = 0;
    for (k, e) in out.iter_mut().enumerate().take(bands.count()) {
        let width = usize::from(bands.width[k]);
        let (window, band) = if k < n_long {
            (None, k)
        } else {
            let s = k - n_long;
            (Some(s % 3), first_short + s / 3)
        };
        *e = Entry {
            start,
            width,
            window,
            band,
        };
        start += width;
    }
    (out, bands.count())
}

/// Intensity stereo for a granule, with mid/side where `ms` says, for the bands it does not
/// code. `g` and `sf` are the right channel's; `lsf` selects the MPEG-2 form.
pub(crate) fn intensity(
    left: &mut [f32; 576],
    right: &mut [f32; 576],
    g: &Granule,
    sf: &Scalefactors,
    lsf: bool,
    ms: bool,
    pan: &Pan,
) {
    let (e, count) = entries(g);
    let e = &e[..count];
    let nonzero = |x: &Entry| right[x.start..x.start + x.width].iter().any(|&v| v != 0.0);
    // Which entries the intensity coding covers.
    let mut coded = [false; 40];
    let n_long = usize::from(g.bands.n_long);
    if g.bands.n_short == 0 {
        let first = e.iter().rposition(nonzero).map_or(0, |k| k + 1);
        coded[first..count].fill(true);
    } else {
        let mut highest_first = 0;
        for w in 0..3 {
            let in_window = |x: &&Entry| x.window == Some(w);
            let first = e
                .iter()
                .enumerate()
                .filter(|(_, x)| in_window(x) && nonzero(x))
                .map(|(_, x)| x.band + 1)
                .max()
                .unwrap_or(if g.mixed { 3 } else { 0 });
            highest_first = highest_first.max(first);
            for (k, x) in e.iter().enumerate() {
                if x.window == Some(w) && x.band >= first {
                    coded[k] = true;
                }
            }
        }
        // A mixed block's long bands are intensity coded only when no short band is non-zero.
        if g.mixed && highest_first <= 3 {
            let first = e[..n_long].iter().rposition(nonzero).map_or(0, |k| k + 1);
            coded[first..n_long].fill(true);
        }
    }
    // Positions; `None` is illegal (not intensity coded after all).
    let legal = |k: usize| -> Option<u8> {
        let v = sf.value[k];
        let illegal = if lsf {
            sf.slen[k] > 0 && u32::from(v) == (1u32 << sf.slen[k]) - 1
        } else {
            v >= 7
        };
        (!illegal).then_some(v)
    };
    let mut pos = [None::<u8>; 40];
    for k in 0..count {
        if !coded[k] {
            continue;
        }
        let last = match e[k].window {
            None => e[k].band == 21,
            Some(_) => e[k].band == 12,
        };
        pos[k] = if last {
            // The entry of the band below in the same window (long: the previous entry).
            let below = if e[k].window.is_some() { k - 3 } else { k - 1 };
            if coded[below] {
                pos[below]
            } else {
                Some(if lsf { 0 } else { 3 })
            }
        } else {
            legal(k)
        };
    }
    let io_shift = if g.scalefac_compress & 1 == 1 { 2 } else { 1 };
    for (k, x) in e.iter().enumerate() {
        let range = x.start..x.start + x.width;
        match pos[k] {
            Some(p) => {
                let (kl, kr) = if lsf {
                    let p = usize::from(p);
                    let k = pan.quarter[(((p + 1) >> 1) * io_shift).min(63)];
                    match p {
                        0 => (1.0, 1.0),
                        _ if p & 1 == 1 => (k, 1.0),
                        _ => (1.0, k),
                    }
                } else {
                    pan.mpeg1[usize::from(p)]
                };
                for (l, r) in left[range.clone()].iter_mut().zip(&mut right[range]) {
                    let v = *l;
                    *l = v * kl;
                    *r = v * kr;
                }
            }
            None if ms => mid_side(&mut left[range.clone()], &mut right[range]),
            None => {}
        }
    }
}
