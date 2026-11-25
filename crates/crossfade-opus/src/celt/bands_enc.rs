//! Band shapes, the encoding half of `celt/bands.c`: `quant_all_bands` with `encode` set and
//! resynthesis on (as the reference's encoder runs it at complexity 8 and up, where the theta
//! search needs it), in the reference's own structure (`quant_band`, `quant_partition`,
//! `quant_band_stereo`, `compute_theta`). What it leaves in the shapes is what the decoder
//! will decode; the folding sources are therefore the decoder's too.
#![allow(clippy::needless_range_loop)] // index loops as the reference writes them

use super::bands::{
    MAX_BAND, compute_qn, deinterleave_hadamard, haar1, interleave_hadamard, stereo_merge,
};
use super::maths::{bitexact_cos, bitexact_log2tan, frac_mul16, lcg_rand};
use super::rate::{BITRES, bits2pulses, get_pulses, pulses2bits, split_threshold};
use super::tables::{EBANDS, LOG_N, NB_EBANDS};
use super::vq::{SPREAD_AGGRESSIVE, alg_quant, renormalise_vector, stereo_itheta};
use crate::range_enc::RangeEncoder;

const QTHETA_OFFSET: i32 = 4;
const QTHETA_OFFSET_TWOPHASE: i32 = 16;

fn eb(i: usize) -> usize {
    EBANDS[i] as usize
}

/// What a band's recursion carries (`struct band_ctx` less the range coder, so that the
/// theta search can save and restore it by copy).
#[derive(Clone, Copy)]
struct Ctx<'a> {
    band: usize,
    intensity: usize,
    spread: i32,
    tf_change: i32,
    remaining_bits: i32,
    band_e: &'a [f32; 2 * NB_EBANDS],
    seed: u32,
    /// 0: round theta to nearest; -1 or 1: round down or up (the theta search).
    theta_round: i32,
    avoid_split_noise: bool,
}

/// `struct split_ctx`.
struct Split {
    inv: bool,
    imid: i32,
    iside: i32,
    delta: i32,
    itheta: i32,
    qalloc: i32,
}

/// `intensity_stereo`: the two channels' mix, weighted by their band amplitudes, into `x`.
fn intensity_stereo(x: &mut [f32], y: &[f32], band_e: &[f32; 2 * NB_EBANDS], band: usize) {
    let left = band_e[band];
    let right = band_e[band + NB_EBANDS];
    let norm = 1e-15 + (1e-15 + left * left + right * right).sqrt();
    let a1 = left / norm;
    let a2 = right / norm;
    for (l, &r) in x.iter_mut().zip(y) {
        *l = a1 * *l + a2 * r;
    }
}

/// `stereo_split`: left and right to mid and side.
fn stereo_split(x: &mut [f32], y: &mut [f32]) {
    const S: f32 = std::f32::consts::FRAC_1_SQRT_2;
    for (a, b) in x.iter_mut().zip(y.iter_mut()) {
        let l = S * *a;
        let r = S * *b;
        *a = l + r;
        *b = r - l;
    }
}

/// `compute_theta` (encoding): chooses, codes and applies the split angle of `x` and `y`.
#[allow(clippy::too_many_arguments)]
fn compute_theta(
    ctx: &mut Ctx<'_>,
    enc: &mut RangeEncoder,
    x: &mut [f32],
    y: &mut [f32],
    n: usize,
    b: &mut i32,
    blocks: usize,
    b0: usize,
    lm: i32,
    stereo: bool,
    fill: &mut u32,
) -> Split {
    let i = ctx.band;
    let pulse_cap = i32::from(LOG_N[i]) + lm * (1 << BITRES);
    let offset = (pulse_cap >> 1)
        - if stereo && n == 2 {
            QTHETA_OFFSET_TWOPHASE
        } else {
            QTHETA_OFFSET
        };
    let mut qn = compute_qn(n as i32, *b, offset, pulse_cap, stereo);
    if stereo && i >= ctx.intensity {
        qn = 1;
    }
    let mut itheta = stereo_itheta(&x[..n], &y[..n], stereo);
    let tell = enc.tell_frac() as i32;
    let mut inv = false;
    if qn != 1 {
        if !stereo || ctx.theta_round == 0 {
            itheta = (itheta * qn + 8192) >> 14;
            if !stereo && ctx.avoid_split_noise && itheta > 0 && itheta < qn {
                // Would the allocation inject noise on one side? Then that side gets nothing.
                let unquantized = itheta * 16384 / qn;
                let imid = bitexact_cos(unquantized);
                let iside = bitexact_cos(16384 - unquantized);
                let delta = frac_mul16(((n - 1) << 7) as i32, bitexact_log2tan(iside, imid));
                if delta > *b {
                    itheta = qn;
                } else if delta < -*b {
                    itheta = 0;
                }
            }
        } else {
            // Biased towards 0 and 16384.
            let bias = if itheta > 8192 {
                32767 / qn
            } else {
                -32767 / qn
            };
            let down = ((itheta * qn + bias) >> 14).clamp(0, qn - 1);
            itheta = if ctx.theta_round < 0 { down } else { down + 1 };
        }
        if stereo && n > 2 {
            let p0 = 3;
            let x0 = qn / 2;
            let ft = p0 * (x0 + 1) + x0;
            let v = itheta;
            let (fl, fh) = if v <= x0 {
                (p0 * v, p0 * (v + 1))
            } else {
                ((v - 1 - x0) + (x0 + 1) * p0, (v - x0) + (x0 + 1) * p0)
            };
            enc.encode(fl as u32, fh as u32, ft as u32);
        } else if b0 > 1 || stereo {
            enc.enc_uint(itheta as u32, (qn + 1) as u32);
        } else {
            let ft = ((qn >> 1) + 1) * ((qn >> 1) + 1);
            let (fs, fl) = if itheta <= qn >> 1 {
                (itheta + 1, (itheta * (itheta + 1)) >> 1)
            } else {
                (
                    qn + 1 - itheta,
                    ft - (((qn + 1 - itheta) * (qn + 2 - itheta)) >> 1),
                )
            };
            enc.encode(fl as u32, (fl + fs) as u32, ft as u32);
        }
        itheta = itheta * 16384 / qn;
        if stereo {
            if itheta == 0 {
                intensity_stereo(&mut x[..n], &y[..n], ctx.band_e, i);
            } else {
                stereo_split(&mut x[..n], &mut y[..n]);
            }
        }
    } else if stereo {
        inv = itheta > 8192;
        if inv {
            for v in &mut y[..n] {
                *v = -*v;
            }
        }
        intensity_stereo(&mut x[..n], &y[..n], ctx.band_e, i);
        if *b > 2 << BITRES && ctx.remaining_bits > 2 << BITRES {
            enc.enc_bit_logp(inv, 2);
        } else {
            inv = false;
        }
        itheta = 0;
    }
    let qalloc = enc.tell_frac() as i32 - tell;
    *b -= qalloc;
    let (imid, iside, delta);
    if itheta == 0 {
        imid = 32767;
        iside = 0;
        *fill &= ((1u64 << blocks) - 1) as u32;
        delta = -16384;
    } else if itheta == 16384 {
        imid = 0;
        iside = 32767;
        *fill &= (((1u64 << blocks) - 1) << blocks) as u32;
        delta = 16384;
    } else {
        imid = bitexact_cos(itheta);
        iside = bitexact_cos(16384 - itheta);
        // The mid/side split of the bits that minimises the squared error.
        delta = frac_mul16(((n - 1) << 7) as i32, bitexact_log2tan(iside, imid));
    }
    Split {
        inv,
        imid,
        iside,
        delta,
        itheta,
        qalloc,
    }
}

/// `quant_band_n1`: a band of one bin per channel is its sign.
fn quant_band_n1(
    ctx: &mut Ctx<'_>,
    enc: &mut RangeEncoder,
    x: &mut [f32],
    y: Option<&mut [f32]>,
    lowband_out: Option<&mut [f32]>,
) -> u32 {
    let mut chans: [Option<&mut [f32]>; 2] = [Some(&mut *x), y];
    for ch in chans.iter_mut().flatten() {
        let mut sign = false;
        if ctx.remaining_bits >= 1 << BITRES {
            sign = ch[0] < 0.0;
            enc.enc_bits(u32::from(sign), 1);
            ctx.remaining_bits -= 1 << BITRES;
        }
        ch[0] = if sign { -1.0 } else { 1.0 };
    }
    if let Some(out) = lowband_out {
        out[0] = x[0];
    }
    1
}

/// `quant_partition`: a mono partition of `n` bins, split in two halves while that pays.
#[allow(clippy::too_many_arguments)]
fn quant_partition(
    ctx: &mut Ctx<'_>,
    enc: &mut RangeEncoder,
    x: &mut [f32],
    mut n: usize,
    mut b: i32,
    mut blocks: usize,
    lowband: Option<&mut [f32]>,
    mut lm: i32,
    gain: f32,
    mut fill: u32,
) -> u32 {
    let b0 = blocks;
    let i = ctx.band;
    let mut cm: u32;
    if lm != -1 && b > split_threshold(i, lm) && n > 2 {
        n >>= 1;
        lm -= 1;
        if blocks == 1 {
            fill = (fill & 1) | (fill << 1);
        }
        blocks = (blocks + 1) >> 1;
        let (xm, ys) = x.split_at_mut(n);
        let s = compute_theta(
            ctx, enc, xm, ys, n, &mut b, blocks, b0, lm, false, &mut fill,
        );
        let mid = (1.0 / 32768.0) * s.imid as f32;
        let side = (1.0 / 32768.0) * s.iside as f32;
        let mut delta = s.delta;
        // More bits to low-energy MDCTs than they would otherwise get.
        if b0 > 1 && (s.itheta & 0x3fff) != 0 {
            if s.itheta > 8192 {
                // A rough approximation of pre-echo masking.
                delta -= delta >> (4 - lm);
            } else {
                // A forward-masking slope of 1.5 dB per 10 ms.
                delta = 0.min(delta + ((n as i32) << BITRES >> (5 - lm)));
            }
        }
        let mut mbits = 0.max(b.min((b - delta) / 2));
        let mut sbits = b - mbits;
        ctx.remaining_bits -= s.qalloc;
        let (lb1, lb2) = match lowband {
            Some(l) => {
                let (a, c) = l.split_at_mut(n.min(l.len()));
                (Some(a), Some(c))
            }
            None => (None, None),
        };
        let mut rebalance = ctx.remaining_bits;
        if mbits >= sbits {
            cm = quant_partition(ctx, enc, xm, n, mbits, blocks, lb1, lm, gain * mid, fill);
            rebalance = mbits - (rebalance - ctx.remaining_bits);
            if rebalance > 3 << BITRES && s.itheta != 0 {
                sbits += rebalance - (3 << BITRES);
            }
            cm |= quant_partition(
                ctx,
                enc,
                ys,
                n,
                sbits,
                blocks,
                lb2,
                lm,
                gain * side,
                fill >> blocks,
            ) << (b0 >> 1);
        } else {
            cm = quant_partition(
                ctx,
                enc,
                ys,
                n,
                sbits,
                blocks,
                lb2,
                lm,
                gain * side,
                fill >> blocks,
            ) << (b0 >> 1);
            rebalance = sbits - (rebalance - ctx.remaining_bits);
            if rebalance > 3 << BITRES && s.itheta != 16384 {
                mbits += rebalance - (3 << BITRES);
            }
            cm |= quant_partition(ctx, enc, xm, n, mbits, blocks, lb1, lm, gain * mid, fill);
        }
    } else {
        let x = &mut x[..n];
        let mut q = bits2pulses(i, lm, b);
        let mut curr_bits = pulses2bits(i, lm, q);
        ctx.remaining_bits -= curr_bits;
        // Never bust the budget.
        while ctx.remaining_bits < 0 && q > 0 {
            ctx.remaining_bits += curr_bits;
            q -= 1;
            curr_bits = pulses2bits(i, lm, q);
            ctx.remaining_bits -= curr_bits;
        }
        if q != 0 {
            let k = get_pulses(q) as usize;
            cm = alg_quant(x, k, ctx.spread, blocks, enc, gain);
        } else {
            // No pulse: fill the band anyway, as the decoder will.
            let cm_mask = ((1u64 << blocks) - 1) as u32;
            fill &= cm_mask;
            cm = 0;
            if fill == 0 {
                x.fill(0.0);
            } else {
                match lowband.as_deref() {
                    None => {
                        for v in x.iter_mut() {
                            ctx.seed = lcg_rand(ctx.seed);
                            *v = ((ctx.seed as i32) >> 20) as f32;
                        }
                        cm = cm_mask;
                    }
                    Some(lb) => {
                        for (j, v) in x.iter_mut().enumerate() {
                            ctx.seed = lcg_rand(ctx.seed);
                            // About 48 dB below the folding level.
                            let tmp = if ctx.seed & 0x8000 != 0 {
                                1.0 / 256.0
                            } else {
                                -1.0 / 256.0
                            };
                            *v = lb.get(j).copied().unwrap_or(0.0) + tmp;
                        }
                        cm = fill;
                    }
                }
                renormalise_vector(x, gain);
            }
        }
    }
    cm
}

/// `quant_band`: a mono band (or one channel of dual stereo, or the mid or side of a stereo
/// band) of `n` bins: the time-frequency changes, then [`quant_partition`], then their undoing.
#[allow(clippy::too_many_arguments)]
fn quant_band(
    ctx: &mut Ctx<'_>,
    enc: &mut RangeEncoder,
    x: &mut [f32],
    n: usize,
    b: i32,
    mut blocks: usize,
    lowband: Option<&[f32]>,
    lm: i32,
    lowband_out: Option<&mut [f32]>,
    gain: f32,
    mut fill: u32,
) -> u32 {
    let n0 = n;
    let mut n_b = n / blocks.max(1);
    let long_blocks = blocks == 1;
    let mut tf_change = ctx.tf_change;
    let mut time_divide = 0;
    if n == 1 {
        return quant_band_n1(ctx, enc, x, None, lowband_out);
    }
    let x = &mut x[..n];
    let recombine = tf_change.max(0);
    // The fold source is transformed with the band: work on a copy.
    let mut scratch = [0.0f32; MAX_BAND];
    let mut lowband: Option<&mut [f32]> = lowband.map(|l| {
        let len = n.min(l.len());
        scratch[..len].copy_from_slice(&l[..len]);
        &mut scratch[..n]
    });
    for k in 0..recombine {
        const BIT_INTERLEAVE: [u32; 16] = [0, 1, 1, 1, 2, 3, 3, 3, 2, 3, 3, 3, 2, 3, 3, 3];
        haar1(x, n >> k, 1 << k);
        if let Some(lb) = lowband.as_deref_mut() {
            haar1(lb, n >> k, 1 << k);
        }
        fill = BIT_INTERLEAVE[(fill & 0xF) as usize]
            | BIT_INTERLEAVE[((fill >> 4) & 0xF) as usize] << 2;
    }
    blocks >>= recombine;
    n_b <<= recombine;
    while n_b & 1 == 0 && tf_change < 0 {
        haar1(x, n_b, blocks);
        if let Some(lb) = lowband.as_deref_mut() {
            haar1(lb, n_b, blocks);
        }
        fill |= fill << blocks;
        blocks <<= 1;
        n_b >>= 1;
        time_divide += 1;
        tf_change += 1;
    }
    let b0 = blocks;
    let n_b0 = n_b;
    if b0 > 1 {
        deinterleave_hadamard(x, n_b >> recombine, b0 << recombine, long_blocks);
        if let Some(lb) = lowband.as_deref_mut() {
            deinterleave_hadamard(lb, n_b >> recombine, b0 << recombine, long_blocks);
        }
    }
    let mut cm = quant_partition(ctx, enc, x, n, b, blocks, lowband, lm, gain, fill);
    if b0 > 1 {
        interleave_hadamard(x, n_b >> recombine, b0 << recombine, long_blocks);
    }
    n_b = n_b0;
    blocks = b0;
    for _ in 0..time_divide {
        blocks >>= 1;
        n_b <<= 1;
        cm |= cm >> blocks;
        haar1(x, n_b, blocks);
    }
    for k in 0..recombine {
        const BIT_DEINTERLEAVE: [u32; 16] = [
            0x00, 0x03, 0x0C, 0x0F, 0x30, 0x33, 0x3C, 0x3F, 0xC0, 0xC3, 0xCC, 0xCF, 0xF0, 0xF3,
            0xFC, 0xFF,
        ];
        cm = BIT_DEINTERLEAVE[(cm & 0xF) as usize];
        haar1(x, n0 >> k, 1 << k);
    }
    blocks <<= recombine;
    if let Some(out) = lowband_out {
        // Scaled for later folding.
        let scale = (n0 as f32).sqrt();
        for (o, &v) in out.iter_mut().zip(x.iter()) {
            *o = scale * v;
        }
    }
    cm & ((1u64 << blocks) - 1) as u32
}

/// `quant_band_stereo`: a stereo band of `n` bins per channel, as mid and side.
#[allow(clippy::too_many_arguments)]
fn quant_band_stereo(
    ctx: &mut Ctx<'_>,
    enc: &mut RangeEncoder,
    x: &mut [f32],
    y: &mut [f32],
    n: usize,
    mut b: i32,
    blocks: usize,
    lowband: Option<&[f32]>,
    lm: i32,
    lowband_out: Option<&mut [f32]>,
    mut fill: u32,
) -> u32 {
    if n == 1 {
        return quant_band_n1(ctx, enc, x, Some(y), lowband_out);
    }
    let orig_fill = fill;
    let s = compute_theta(
        ctx, enc, x, y, n, &mut b, blocks, blocks, lm, true, &mut fill,
    );
    let mid = (1.0 / 32768.0) * s.imid as f32;
    let side = (1.0 / 32768.0) * s.iside as f32;
    let mut cm;
    if n == 2 {
        // The side is orthogonal to the mid: one bit of sign codes it.
        let mut mbits = b;
        let mut sbits = 0;
        if s.itheta != 0 && s.itheta != 16384 {
            sbits = 1 << BITRES;
        }
        mbits -= sbits;
        let c = s.itheta > 8192;
        ctx.remaining_bits -= s.qalloc + sbits;
        let (x2, y2): (&mut [f32], &mut [f32]) = if c {
            (&mut *y, &mut *x)
        } else {
            (&mut *x, &mut *y)
        };
        let mut sign = 0;
        if sbits != 0 {
            sign = i32::from(x2[0] * y2[1] - x2[1] * y2[0] < 0.0);
            enc.enc_bits(sign as u32, 1);
        }
        let sign = (1 - 2 * sign) as f32;
        // orig_fill: the side folds, even where itheta 16384 cleared fill's low bits.
        cm = quant_band(
            ctx,
            enc,
            x2,
            n,
            mbits,
            blocks,
            lowband,
            lm,
            lowband_out,
            1.0,
            orig_fill,
        );
        y2[0] = -sign * x2[1];
        y2[1] = sign * x2[0];
        x[0] *= mid;
        x[1] *= mid;
        y[0] *= side;
        y[1] *= side;
        let tmp = x[0];
        x[0] = tmp - y[0];
        y[0] += tmp;
        let tmp = x[1];
        x[1] = tmp - y[1];
        y[1] += tmp;
    } else {
        let mut mbits = 0.max(b.min((b - s.delta) / 2));
        let mut sbits = b - mbits;
        ctx.remaining_bits -= s.qalloc;
        let mut rebalance = ctx.remaining_bits;
        // The mid is not scaled: folding needs it normalised.
        if mbits >= sbits {
            cm = quant_band(
                ctx,
                enc,
                x,
                n,
                mbits,
                blocks,
                lowband,
                lm,
                lowband_out,
                1.0,
                fill,
            );
            rebalance = mbits - (rebalance - ctx.remaining_bits);
            if rebalance > 3 << BITRES && s.itheta != 0 {
                sbits += rebalance - (3 << BITRES);
            }
            // fill's high bits are zero for a stereo split: the side never folds.
            cm |= quant_band(
                ctx,
                enc,
                y,
                n,
                sbits,
                blocks,
                None,
                lm,
                None,
                side,
                fill >> blocks,
            );
        } else {
            cm = quant_band(
                ctx,
                enc,
                y,
                n,
                sbits,
                blocks,
                None,
                lm,
                None,
                side,
                fill >> blocks,
            );
            rebalance = sbits - (rebalance - ctx.remaining_bits);
            if rebalance > 3 << BITRES && s.itheta != 16384 {
                mbits += rebalance - (3 << BITRES);
            }
            cm |= quant_band(
                ctx,
                enc,
                x,
                n,
                mbits,
                blocks,
                lowband,
                lm,
                lowband_out,
                1.0,
                fill,
            );
        }
    }
    if n != 2 {
        stereo_merge(&mut x[..n], &mut y[..n], mid);
    }
    if s.inv {
        for v in &mut y[..n] {
            *v = -*v;
        }
    }
    cm
}

/// What [`quant_all_bands_enc`] needs beyond the shapes.
pub(super) struct EncodeBands<'a> {
    pub(super) start: usize,
    pub(super) end: usize,
    /// The band amplitudes (for intensity stereo and the theta search's weights).
    pub(super) band_e: &'a [f32; 2 * NB_EBANDS],
    pub(super) pulses: &'a [i32; NB_EBANDS],
    pub(super) short_blocks: bool,
    pub(super) spread: i32,
    pub(super) dual_stereo: bool,
    pub(super) intensity: usize,
    pub(super) tf_res: &'a [i32; NB_EBANDS],
    pub(super) total_bits: i32,
    pub(super) balance: i32,
    pub(super) lm: usize,
    pub(super) coded_bands: usize,
    /// Code each stereo band below intensity with theta rounded down and up, and keep the
    /// better (complexity 8 and up).
    pub(super) theta_rdo: bool,
}

/// `special_hybrid_folding` (RFC 8251 section 9): repeat part of the first band so that the
/// second can fold; copies nothing in CELT-only mode.
fn special_hybrid_folding(norm: &mut [f32], norm2: &mut [f32], start: usize, m: usize, dual: bool) {
    let n1 = m * (eb(start + 1) - eb(start));
    let n2 = m * (eb(start + 2) - eb(start + 1));
    let offset = m * eb(start);
    if n2 > n1 && 2 * n1 >= n2 {
        let from = offset + 2 * n1 - n2;
        norm.copy_within(from..from + n2 - n1, offset + n1);
        if dual {
            norm2.copy_within(from..from + n2 - n1, offset + n1);
        }
    }
}

/// `compute_channel_weights`: the amplitudes, a little evened out, weigh each channel's
/// distortion in the theta search.
fn channel_weights(ex: f32, ey: f32) -> [f32; 2] {
    let min_e = ex.min(ey);
    [ex + min_e / 3.0, ey + min_e / 3.0]
}

fn inner(a: &[f32], b: &[f32]) -> f32 {
    let mut s = 0.0f32;
    for (&x, &y) in a.iter().zip(b) {
        s += x * y;
    }
    s
}

/// `quant_all_bands` (encoding): codes every band's shape in `x` (and `y`), `n` bins per
/// channel, leaving the shapes the decoder will decode; the collapse masks go to
/// `collapse_masks` (`band * c + channel`), `seed` is the folding noise's LCG state.
pub(super) fn quant_all_bands_enc(
    a: &EncodeBands<'_>,
    x: &mut [f32],
    mut y: Option<&mut [f32]>,
    collapse_masks: &mut [u8; 2 * NB_EBANDS],
    enc: &mut RangeEncoder,
    seed: &mut u32,
) {
    let c = if y.is_some() { 2 } else { 1 };
    let m = 1usize << a.lm;
    let blocks = if a.short_blocks { m } else { 1 };
    let norm_len = (m * eb(NB_EBANDS)).min(800);
    let mut norm_buf = [0.0f32; 2 * 800];
    let (norm, norm2) = norm_buf.split_at_mut(800);
    let norm = &mut norm[..norm_len];
    let norm2 = &mut norm2[..norm_len];
    let theta_rdo = a.theta_rdo && c == 2 && !a.dual_stereo;
    let mut lowband_offset = 0usize;
    let mut update_lowband = true;
    let mut dual_stereo = a.dual_stereo;
    let mut balance = a.balance;
    let mut ctx = Ctx {
        band: 0,
        intensity: a.intensity,
        spread: a.spread,
        tf_change: 0,
        remaining_bits: 0,
        band_e: a.band_e,
        seed: *seed,
        theta_round: 0,
        // No noise injected in the first band on transients.
        avoid_split_noise: blocks > 1,
    };
    let mut fold = [0.0f32; MAX_BAND];
    for i in a.start..a.end {
        let lo = m * eb(i);
        let n = m * eb(i + 1) - lo;
        let last = i == a.end - 1;
        let tell = enc.tell_frac() as i32;
        if i != a.start {
            balance -= tell;
        }
        let remaining_bits = a.total_bits - tell - 1;
        ctx.remaining_bits = remaining_bits;
        ctx.band = i;
        let b = if i < a.coded_bands {
            let curr_balance = balance / 3.min((a.coded_bands - i) as i32);
            0.max(16383.min((remaining_bits + 1).min(a.pulses[i] + curr_balance)))
        } else {
            0
        };
        if (lo as isize - n as isize >= (m * eb(a.start)) as isize || i == a.start + 1)
            && (update_lowband || lowband_offset == 0)
        {
            lowband_offset = i;
        }
        if i == a.start + 1 {
            special_hybrid_folding(norm, norm2, a.start, m, dual_stereo);
        }
        ctx.tf_change = a.tf_res[i];
        // A conservative estimate of the collapse masks of the bands folded from.
        let mut effective_lowband: Option<usize> = None;
        let (mut x_cm, mut y_cm);
        if lowband_offset != 0 && (a.spread != SPREAD_AGGRESSIVE || blocks > 1 || ctx.tf_change < 0)
        {
            let eff = (m * eb(a.start)).max((m * eb(lowband_offset)).saturating_sub(n));
            effective_lowband = Some(eff);
            let mut fold_start = lowband_offset;
            loop {
                fold_start -= 1;
                if fold_start == 0 || m * eb(fold_start) <= eff {
                    break;
                }
            }
            let mut fold_end = lowband_offset - 1;
            loop {
                fold_end += 1;
                if !(fold_end < i && m * eb(fold_end) < eff + n) {
                    break;
                }
            }
            x_cm = 0u32;
            y_cm = 0u32;
            let mut fold_i = fold_start;
            loop {
                x_cm |= u32::from(collapse_masks[fold_i * c]);
                y_cm |= u32::from(collapse_masks[fold_i * c + c - 1]);
                fold_i += 1;
                if fold_i >= fold_end {
                    break;
                }
            }
        } else {
            // Folding from the LCG: every block (almost always) gets something.
            x_cm = ((1u64 << blocks) - 1) as u32;
            y_cm = x_cm;
        }
        if dual_stereo && i == a.intensity {
            // Dual stereo ends where intensity begins.
            dual_stereo = false;
            for j in m * eb(a.start)..lo {
                norm[j] = 0.5 * (norm[j] + norm2[j]);
            }
        }
        // The fold source is copied first: it may overlap this band's output (hybrid).
        let lowband = |src: &[f32], fold: &mut [f32; MAX_BAND]| -> bool {
            match effective_lowband {
                Some(e) if e + n <= src.len() => {
                    fold[..n].copy_from_slice(&src[e..e + n]);
                    true
                }
                _ => false,
            }
        };
        let xb = &mut x[lo..lo + n];
        let lm = a.lm as i32;
        if dual_stereo {
            let yb = &mut y.as_deref_mut().expect("dual stereo has y")[lo..lo + n];
            let has = lowband(norm, &mut fold);
            let out = (!last).then_some(&mut norm[lo..lo + n]);
            x_cm = quant_band(
                &mut ctx,
                enc,
                xb,
                n,
                b / 2,
                blocks,
                has.then_some(&fold[..n]),
                lm,
                out,
                1.0,
                x_cm,
            );
            let has = lowband(norm2, &mut fold);
            let out = (!last).then_some(&mut norm2[lo..lo + n]);
            y_cm = quant_band(
                &mut ctx,
                enc,
                yb,
                n,
                b / 2,
                blocks,
                has.then_some(&fold[..n]),
                lm,
                out,
                1.0,
                y_cm,
            );
        } else if let Some(yall) = y.as_deref_mut() {
            let yb = &mut yall[lo..lo + n];
            let has = lowband(norm, &mut fold);
            let lb = has.then_some(&fold[..n]);
            if theta_rdo && i < a.intensity {
                let w = channel_weights(a.band_e[i], a.band_e[i + NB_EBANDS]);
                let cm = x_cm | y_cm;
                let enc_save = enc.clone();
                let ctx_save = ctx;
                let mut x_save = [0.0f32; MAX_BAND];
                let mut y_save = [0.0f32; MAX_BAND];
                x_save[..n].copy_from_slice(xb);
                y_save[..n].copy_from_slice(yb);
                // Round down.
                ctx.theta_round = -1;
                let out = (!last).then_some(&mut norm[lo..lo + n]);
                let cm_down =
                    quant_band_stereo(&mut ctx, enc, xb, yb, n, b, blocks, lb, lm, out, cm);
                let dist_down = w[0] * inner(&x_save[..n], xb) + w[1] * inner(&y_save[..n], yb);
                let enc_down = std::mem::replace(enc, enc_save);
                let ctx_down = ctx;
                let mut x_down = [0.0f32; MAX_BAND];
                let mut y_down = [0.0f32; MAX_BAND];
                let mut norm_down = [0.0f32; MAX_BAND];
                x_down[..n].copy_from_slice(xb);
                y_down[..n].copy_from_slice(yb);
                if !last {
                    norm_down[..n].copy_from_slice(&norm[lo..lo + n]);
                }
                ctx = ctx_save;
                xb.copy_from_slice(&x_save[..n]);
                yb.copy_from_slice(&y_save[..n]);
                if i == a.start + 1 {
                    special_hybrid_folding(norm, norm2, a.start, m, dual_stereo);
                }
                // Round up.
                ctx.theta_round = 1;
                let out = (!last).then_some(&mut norm[lo..lo + n]);
                x_cm = quant_band_stereo(&mut ctx, enc, xb, yb, n, b, blocks, lb, lm, out, cm);
                let dist_up = w[0] * inner(&x_save[..n], xb) + w[1] * inner(&y_save[..n], yb);
                if dist_down >= dist_up {
                    x_cm = cm_down;
                    *enc = enc_down;
                    ctx = ctx_down;
                    xb.copy_from_slice(&x_down[..n]);
                    yb.copy_from_slice(&y_down[..n]);
                    if !last {
                        norm[lo..lo + n].copy_from_slice(&norm_down[..n]);
                    }
                }
            } else {
                ctx.theta_round = 0;
                let out = (!last).then_some(&mut norm[lo..lo + n]);
                x_cm = quant_band_stereo(
                    &mut ctx,
                    enc,
                    xb,
                    yb,
                    n,
                    b,
                    blocks,
                    lb,
                    lm,
                    out,
                    x_cm | y_cm,
                );
            }
            y_cm = x_cm;
        } else {
            let has = lowband(norm, &mut fold);
            let out = (!last).then_some(&mut norm[lo..lo + n]);
            x_cm = quant_band(
                &mut ctx,
                enc,
                xb,
                n,
                b,
                blocks,
                has.then_some(&fold[..n]),
                lm,
                out,
                1.0,
                x_cm | y_cm,
            );
            y_cm = x_cm;
        }
        collapse_masks[i * c] = x_cm as u8;
        collapse_masks[i * c + c - 1] = y_cm as u8;
        balance += a.pulses[i] + tell;
        // The folding position moves on only while there is a bit per sample.
        update_lowband = b > (n as i32) << BITRES;
        // Only the first band needs to avoid noise on a split; folding takes over after.
        ctx.avoid_split_noise = false;
    }
    *seed = ctx.seed;
}

#[cfg(test)]
#[path = "bands_enc_tests.rs"]
mod tests;
