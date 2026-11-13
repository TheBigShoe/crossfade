//! The 32-band polyphase synthesis filterbank (ISO/IEC 11172-3 2.4.3.2.2 and Annex A): a
//! 32-point DCT-II per time slot, then the 512-tap window over the last 16 slots.
//!
//! The fast DCT-II and its constants follow minimp3 (public domain, CC0 1.0) by way of
//! nanomp3-core (MIT OR Apache-2.0, Copyright (c) 2025 Robert B. Langer). The windowing is the
//! standard's in matrix form. A slot's 64-value vector V comes from the DCT's 32 outputs A by
//! symmetry: V[j] = A[16 + j], 0 at j = 16, -A[48 - j]; V[32 + j] = -A[16 - j], -A[j - 16].
//! Sample j of a slot is the sum over the last 16 slots of a window coefficient times the
//! first half of their V (even look-backs) or the second (odd ones), and samples j and 32 - j
//! take the same element of A: so each slot keeps two short rows, E = A[16..32] then 0 and
//! O = A[16], A[15], ..., A[0], and samples 0..16 read them from element 0, samples 31 down to
//! 16 from element 1, with the signs in the window. Every sum is then contiguous 4-wide
//! multiply-adds in a fixed order: the same bits everywhere.
#![allow(clippy::excessive_precision)]

use crate::lanes::F4;
use crate::tables::{DCT_II_SEC, SYNTH_WINDOW};

/// The output's bound: samples are clamped to plus or minus this, so any input gives finite,
/// bounded output (+12 dB over full scale leaves real overshoot alone).
pub const OUTPUT_LIMIT: f32 = 4.0;

/// A slot's two rows, E and O, 17 values each (padded to 20).
type Slot = [[f32; 20]; 2];
/// Slots kept per channel from one granule to the next.
const HISTORY: usize = 15;
/// Slots per channel while a granule is synthesised: the history, then its 18.
const SLOTS: usize = HISTORY + 18;

/// The window over 65536 and the output's 16-bit scale over 32768 (one exact power of two),
/// with each row's samples 16..32 reversed to match the rows' order: column `16 + k` is
/// sample `31 - k`.
const WINDOW: [[f32; 32]; 16] = {
    let mut w = [[0.0; 32]; 16];
    let mut m = 0;
    while m < 16 {
        let mut j = 0;
        while j < 32 {
            let from = if j < 16 { j } else { 47 - j };
            w[m][j] = SYNTH_WINDOW[m][from] * (1.0 / 32768.0);
            j += 1;
        }
        m += 1;
    }
    w
};

/// The filterbank's state: per channel, the rows of the last 15 time slots.
pub(crate) struct Synth {
    slots: Box<[[Slot; SLOTS]; 2]>,
}

impl Synth {
    pub(crate) fn new() -> Self {
        Self {
            slots: Box::new([[[[0.0; 20]; 2]; SLOTS]; 2]),
        }
    }

    pub(crate) fn reset(&mut self) {
        for ch in self.slots.iter_mut() {
            ch[..HISTORY].fill([[0.0; 20]; 2]);
        }
    }

    /// One granule: `x` holds `nch` channels of 576 subband samples (subband by slot); `pcm`
    /// receives 576 interleaved sample frames. A mono granule moves only the first channel's
    /// history on.
    pub(crate) fn granule(&mut self, x: &[[f32; 576]; 2], nch: usize, pcm: &mut [f32]) {
        for (x, slots) in x.iter().zip(self.slots.iter_mut()).take(nch) {
            let new = &mut slots[HISTORY..];
            // 18 slots in groups of four; the last group overlaps the one before and writes
            // the same values for slots 14 and 15.
            dct_ii::<0>(x, new);
            dct_ii::<4>(x, new);
            dct_ii::<8>(x, new);
            dct_ii::<12>(x, new);
            dct_ii::<14>(x, new);
        }
        if nch == 1 {
            blocks::<1>(&self.slots, pcm);
        } else {
            blocks::<2>(&self.slots, pcm);
        }
        for slots in self.slots.iter_mut().take(nch) {
            slots.copy_within(SLOTS - HISTORY.., 0);
        }
    }
}

fn blocks<const NCH: usize>(slots: &[[Slot; SLOTS]; 2], pcm: &mut [f32]) {
    window::<NCH, 0>(slots, pcm);
    window::<NCH, 4>(slots, pcm);
    window::<NCH, 8>(slots, pcm);
    window::<NCH, 12>(slots, pcm);
    window::<NCH, 16>(slots, pcm);
    window::<NCH, 20>(slots, pcm);
    window::<NCH, 24>(slots, pcm);
    window::<NCH, 28>(slots, pcm);
}

/// Column `COL..COL + 4` of every slot's samples (0..16, then 31 down to 16): the window's row
/// `m` times a row of slot `t - m` (E for even `m`, O for odd), summed from `m = 0` up in two
/// chains, the even and the odd look-backs, added at the end. The column's 16 window vectors
/// stay in registers for all slots and channels.
#[inline(always)]
fn window<const NCH: usize, const COL: usize>(slots: &[[Slot; SLOTS]; 2], pcm: &mut [f32]) {
    let at = COL % 16 + COL / 16;
    let w: [F4; 16] = core::array::from_fn(|m| F4::load(&WINDOW[m][COL..]));
    for t in 0..18 {
        let mut out = [F4::splat(0.0); NCH];
        for (o, slots) in out.iter_mut().zip(slots) {
            let Some(last) = slots[t..].first_chunk::<16>() else {
                return;
            };
            let row = |back: usize| F4::load(&last[15 - back][back & 1][at..]);
            let mut even = w[0] * row(0);
            let mut odd = w[1] * row(1);
            for k in 1..8 {
                even += w[2 * k] * row(2 * k);
                odd += w[2 * k + 1] * row(2 * k + 1);
            }
            *o = even + odd;
        }
        // Samples 31 down to 16 go out reversed.
        let first = if COL < 16 {
            COL
        } else {
            for o in &mut out {
                *o = o.rev();
            }
            44 - COL
        };
        let dst = &mut pcm[NCH * (32 * t + first)..][..4 * NCH];
        for (i, d) in dst.iter_mut().enumerate() {
            *d = out_sample(out[i % NCH].0[i / NCH]);
        }
    }
}

/// The DCT-II of slots `K..K + 4` of the 18x32 (slot by subband) matrix, each slot's outputs
/// `A` stored as its rows E and O.
#[inline(always)]
#[allow(clippy::assign_op_pattern)]
fn dct_ii<const K: usize>(x: &[f32; 576], slots: &mut [Slot]) {
    let k = K;
    let c = F4::splat;
    let mut t = [[c(0.0); 8]; 4];
    for i in 0..8 {
        let x0 = F4::load(&x[k + i * 18..]);
        let x1 = F4::load(&x[k + (15 - i) * 18..]);
        let x2 = F4::load(&x[k + (16 + i) * 18..]);
        let x3 = F4::load(&x[k + (31 - i) * 18..]);
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
    let a = |n: usize| -> F4 {
        let i = n / 4;
        match (n % 4, i) {
            (0, _) => t[0][i],
            (1, 7) => t[2][7] + t[3][7],
            (1, _) => t[2][i] + t[3][i] + t[3][i + 1],
            (2, 7) => t[1][7],
            (2, _) => t[1][i] + t[1][i + 1],
            (_, 7) => t[3][7],
            _ => t[2][i + 1] + t[3][i] + t[3][i + 1],
        }
    };
    // Four outputs of four slots at a time, turned into four outputs of each slot.
    let mut blocks = [[F4::splat(0.0); 8]; 4];
    for b in 0..8 {
        let v = [a(4 * b), a(4 * b + 1), a(4 * b + 2), a(4 * b + 3)];
        for (lane, slot) in blocks.iter_mut().enumerate() {
            slot[b] = F4([v[0].0[lane], v[1].0[lane], v[2].0[lane], v[3].0[lane]]);
        }
    }
    for (b, [e, o]) in blocks.iter().zip(&mut slots[K..K + 4]) {
        for i in 0..4 {
            b[4 + i].store(&mut e[4 * i..]);
            b[3 - i].rev().store(&mut o[1 + 4 * i..]);
        }
        e[16] = 0.0;
        o[0] = b[4].0[0];
    }
}

/// `max` then `min` rather than `clamp`: a NaN (which finite input never gives, but the bound
/// is a promise) becomes `-OUTPUT_LIMIT` instead of passing through.
#[inline(always)]
#[allow(clippy::manual_clamp)]
fn out_sample(x: f32) -> f32 {
    x.max(-OUTPUT_LIMIT).min(OUTPUT_LIMIT)
}
