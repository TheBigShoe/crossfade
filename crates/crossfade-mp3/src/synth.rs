//! The 32-band polyphase synthesis filterbank (ISO/IEC 11172-3 2.4.3.2.2 and Annex A): a
//! 32-point DCT-II per time slot, then the 512-tap window over the last 16 slots.
//!
//! The algorithm (a fast DCT-II, and a window walk that keeps both channels' even and odd
//! slots as four lanes) and its constants follow minimp3 (public domain, CC0 1.0) by way of
//! nanomp3-core (MIT OR Apache-2.0, Copyright (c) 2025 Robert B. Langer). The window is the
//! standard's times 65536 and output comes out at 16-bit scale, so it is scaled by 1/32768.
#![allow(clippy::excessive_precision)]

use crate::lanes::{F4, Lane};
use crate::tables::{DCT_II_SEC, SYNTH_WIN};

/// The output's bound: samples are clamped to plus or minus this, so any input gives finite,
/// bounded output (+12 dB over full scale leaves real overshoot alone).
pub const OUTPUT_LIMIT: f32 = 4.0;

/// Floats of filterbank history kept between granules.
pub(crate) const HISTORY: usize = 15 * 64;

/// Rows of four floats in the working buffer: the history, then two rows per time slot.
pub(crate) const ROWS: usize = (18 + 15) * 2 * 32 / 4;

const ZLIN: usize = 15 * 16;
const SYNTH_ROWS: usize = ZLIN + 2 * 16;

/// DCT-II over columns `k..k + T`'s lanes of the 18x32 (slot by subband) matrix.
#[inline(always)]
fn dct_ii_cols<T: Lane + LoadStore>(x: &mut [f32; 576], k: usize) {
    let c = T::splat;
    let mut t = [[c(0.0); 8]; 4];
    for i in 0..8 {
        let x0 = T::load(&x[k + i * 18..]);
        let x1 = T::load(&x[k + (15 - i) * 18..]);
        let x2 = T::load(&x[k + (16 + i) * 18..]);
        let x3 = T::load(&x[k + (31 - i) * 18..]);
        let t0 = x0 + x3;
        let t1 = x1 + x2;
        let t2 = (x1 - x2) * c(DCT_II_SEC[3 * i]);
        let t3 = (x0 - x3) * c(DCT_II_SEC[3 * i + 1]);
        t[0][i] = t0 + t1;
        t[1][i] = (t0 - t1) * c(DCT_II_SEC[3 * i + 2]);
        t[2][i] = t3 + t2;
        t[3][i] = (t3 - t2) * c(DCT_II_SEC[3 * i + 2]);
    }
    for x in &mut t {
        let [
            mut x0,
            mut x1,
            mut x2,
            mut x3,
            mut x4,
            mut x5,
            mut x6,
            mut x7,
        ] = *x;
        let mut xt = x0 - x7;
        x0 = x0 + x7;
        x7 = x1 - x6;
        x1 = x1 + x6;
        x6 = x2 - x5;
        x2 = x2 + x5;
        x5 = x3 - x4;
        x3 = x3 + x4;
        x4 = x0 - x3;
        x0 = x0 + x3;
        x3 = x1 - x2;
        x1 = x1 + x2;
        x[0] = x0 + x1;
        x[4] = (x0 - x1) * c(0.70710677);
        x5 = x5 + x6;
        x6 = (x6 + x7) * c(0.70710677);
        x7 = x7 + xt;
        x3 = (x3 + x4) * c(0.70710677);
        x5 = x5 - x7 * c(0.198912367);
        x7 = x7 + x5 * c(0.382683432);
        x5 = x5 - x7 * c(0.198912367);
        x0 = xt - x6;
        xt = xt + x6;
        x[1] = (xt + x7) * c(0.50979561);
        x[2] = (x4 + x3) * c(0.54119611);
        x[3] = (x0 - x5) * c(0.60134488);
        x[5] = (x0 + x5) * c(0.89997619);
        x[6] = (x4 - x3) * c(1.30656302);
        x[7] = (xt - x7) * c(2.56291556);
    }
    for i in 0..7 {
        let y = k + i * 4 * 18;
        t[0][i].store(&mut x[y..]);
        (t[2][i] + t[3][i] + t[3][i + 1]).store(&mut x[y + 18..]);
        (t[1][i] + t[1][i + 1]).store(&mut x[y + 2 * 18..]);
        (t[2][i + 1] + t[3][i] + t[3][i + 1]).store(&mut x[y + 3 * 18..]);
    }
    let y = k + 7 * 4 * 18;
    t[0][7].store(&mut x[y..]);
    (t[2][7] + t[3][7]).store(&mut x[y + 18..]);
    t[1][7].store(&mut x[y + 2 * 18..]);
    t[3][7].store(&mut x[y + 3 * 18..]);
}

/// Contiguous loads and stores (unlike `Lane`'s strided ones).
pub(crate) trait LoadStore {
    fn load(s: &[f32]) -> Self;
    fn store(self, s: &mut [f32]);
}

impl LoadStore for f32 {
    #[inline(always)]
    fn load(s: &[f32]) -> Self {
        s[0]
    }
    #[inline(always)]
    fn store(self, s: &mut [f32]) {
        s[0] = self;
    }
}

impl LoadStore for F4 {
    #[inline(always)]
    fn load(s: &[f32]) -> Self {
        F4::load(s)
    }
    #[inline(always)]
    fn store(self, s: &mut [f32]) {
        F4::store(self, s);
    }
}

fn dct_ii(x: &mut [f32; 576]) {
    // 18 slots: four groups of four lanes and two single columns.
    for k in [0, 4, 8, 12] {
        dct_ii_cols::<F4>(x, k);
    }
    dct_ii_cols::<f32>(x, 16);
    dct_ii_cols::<f32>(x, 17);
}

/// `max` then `min` rather than `clamp`: a NaN (which finite input never gives, but the bound
/// is a promise) becomes `-OUTPUT_LIMIT` instead of passing through.
#[inline(always)]
#[allow(clippy::manual_clamp)]
fn out_sample(x: f32) -> f32 {
    (x * (1.0 / 32768.0)).max(-OUTPUT_LIMIT).min(OUTPUT_LIMIT)
}

/// The two samples written to `pcm[0]` and `pcm[16 * nch]`, from `lane` (and `lane + 2`) of
/// every 16th row from `row`.
#[inline(always)]
fn synth_pair(z: &[[f32; 4]; SYNTH_ROWS], row: usize, lane: usize) -> (f32, f32) {
    let x = |m: usize| z[row + 16 * m][lane];
    let mut a;
    a = (x(14) - x(0)) * 29.0;
    a += (x(1) + x(13)) * 213.0;
    a += (x(12) - x(2)) * 459.0;
    a += (x(3) + x(11)) * 2037.0;
    a += (x(10) - x(4)) * 5153.0;
    a += (x(5) + x(9)) * 6574.0;
    a += (x(8) - x(6)) * 37489.0;
    a += x(7) * 75038.0;
    let first = a;
    let x = |m: usize| z[row + 16 * m][lane + 2];
    a = x(14) * 104.0;
    a += x(12) * 1567.0;
    a += x(10) * 9727.0;
    a += x(8) * 64019.0;
    a += x(6) * -9975.0;
    a += x(4) * -45.0;
    a += x(2) * 146.0;
    a += x(0) * -5.0;
    (first, a)
}

/// Two time slots (64 samples per channel) from the DCT outputs `xl` and `xr`.
fn synth<const NCH: usize>(xl: &[f32], xr: &[f32], dst: &mut [f32], lins: &mut [[f32; 4]]) {
    let dst = &mut dst[..64 * NCH];
    let Some(lins) = lins.first_chunk_mut::<SYNTH_ROWS>() else {
        return;
    };
    let (Some(xl), Some(xr)) = (xl.first_chunk::<560>(), xr.first_chunk::<560>()) else {
        return;
    };
    let r = NCH - 1;
    lins[ZLIN + 15] = [xl[18 * 16], xr[18 * 16], xl[0], xr[0]];
    lins[ZLIN + 31] = [xl[1 + 18 * 16], xr[1 + 18 * 16], xl[1], xr[1]];
    let mut pair = |at: usize, row: usize, lane: usize, lins: &[[f32; 4]; SYNTH_ROWS]| {
        let (a, b) = synth_pair(lins, row, lane);
        dst[at] = out_sample(a);
        dst[at + 16 * NCH] = out_sample(b);
    };
    if NCH == 2 {
        pair(r, 15, 1, lins);
        pair(r + 32 * NCH, 31, 1, lins);
    }
    pair(0, 15, 0, lins);
    pair(32 * NCH, 31, 0, lins);

    for i in (0..15).rev() {
        lins[ZLIN + i] = [
            xl[18 * (31 - i)],
            xr[18 * (31 - i)],
            xl[1 + 18 * (31 - i)],
            xr[1 + 18 * (31 - i)],
        ];
        lins[ZLIN + 16 + i][0] = xl[1 + 18 * (1 + i)];
        lins[ZLIN + 16 + i][1] = xr[1 + 18 * (1 + i)];
        lins[ZLIN - 16 + i][2] = xl[18 * (1 + i)];
        lins[ZLIN - 16 + i][3] = xr[18 * (1 + i)];

        let w = &SYNTH_WIN[(14 - i) * 16..][..16];
        let vz = |k: usize| F4(lins[ZLIN + i - 16 * k]);
        let vy = |k: usize| F4(lins[i + 16 * k]);
        let w0 = |k: usize| F4::splat(w[2 * k]);
        let w1 = |k: usize| F4::splat(w[2 * k + 1]);
        let mut b = vz(0) * w1(0) + vy(0) * w0(0);
        let mut a = vz(0) * w0(0) - vy(0) * w1(0);
        for k in [1, 3, 5, 7] {
            b += vz(k) * w1(k) + vy(k) * w0(k);
            a += vy(k) * w1(k) - vz(k) * w0(k);
            if k < 7 {
                let k = k + 1;
                b += vz(k) * w1(k) + vy(k) * w0(k);
                a += vz(k) * w0(k) - vy(k) * w1(k);
            }
        }
        let (a, b) = (a.0, b.0);
        if NCH == 2 {
            dst[r + (15 - i) * NCH] = out_sample(a[1]);
            dst[r + (17 + i) * NCH] = out_sample(b[1]);
        }
        dst[(15 - i) * NCH] = out_sample(a[0]);
        dst[(17 + i) * NCH] = out_sample(b[0]);
        if NCH == 2 {
            dst[r + (47 - i) * NCH] = out_sample(a[3]);
            dst[r + (49 + i) * NCH] = out_sample(b[3]);
        }
        dst[(47 - i) * NCH] = out_sample(a[2]);
        dst[(49 + i) * NCH] = out_sample(b[2]);
    }
}

fn synth_all<const NCH: usize>(x: &[f32], pcm: &mut [f32], lins: &mut [[f32; 4]; ROWS]) {
    for i in (0..18).step_by(2) {
        synth::<NCH>(
            &x[i..],
            &x[576 * (NCH - 1) + i..],
            &mut pcm[32 * NCH * i..],
            &mut lins[16 * i..],
        );
    }
}

/// One granule: `x` holds `nch` channels of 576 subband samples (subband by slot), which are
/// overwritten; `pcm` receives 576 interleaved sample frames. `history` carries the filterbank
/// state from granule to granule.
pub(crate) fn granule(
    history: &mut [f32; HISTORY],
    x: &mut [[f32; 576]; 2],
    nch: usize,
    pcm: &mut [f32],
    lins: &mut [[f32; 4]; ROWS],
) {
    for ch in x.iter_mut().take(nch) {
        dct_ii(ch);
    }
    lins.as_flattened_mut()[..HISTORY].copy_from_slice(history);
    let flat = x.as_flattened();
    if nch == 1 {
        synth_all::<1>(flat, pcm, lins);
    } else {
        synth_all::<2>(flat, pcm, lins);
    }
    let tail = &lins.as_flattened()[18 * 64..18 * 64 + HISTORY];
    if nch == 1 {
        // A mono granule moves only the left channel's history on.
        for (h, t) in history
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .zip(tail.as_chunks::<2>().0)
        {
            h[0] = t[0];
        }
    } else {
        history.copy_from_slice(tail);
    }
}
