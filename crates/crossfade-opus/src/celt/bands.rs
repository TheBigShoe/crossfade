//! Band shapes (`celt/bands.c`, the decoding half): `quant_all_bands` and its recursive
//! `quant_band` (splitting, stereo angles, folding, the time-frequency transforms), the
//! anti-collapse noise and denormalisation. RFC 8251 section 9's hybrid folding is applied.
#![allow(clippy::needless_range_loop)] // index loops as the reference writes them

use super::ec::Ec;
use super::maths::{bitexact_cos, bitexact_log2tan, exp2, frac_mul16, isqrt32, lcg_rand};
use super::rate::{BITRES, bits2pulses, get_pulses, pulses2bits, split_threshold};
use super::tables::{EBANDS, LOG_N, NB_EBANDS, ORDERY_TABLE};
use super::vq::{SPREAD_AGGRESSIVE, alg_unquant, renormalise_vector};

const QTHETA_OFFSET: i32 = 4;
const QTHETA_OFFSET_TWOPHASE: i32 = 16;
/// The longest band at 20 ms: 22 bins of 2.5 ms times 8.
pub(super) const MAX_BAND: usize = 176;

fn eb(i: usize) -> usize {
    EBANDS[i] as usize
}

/// `haar1`: a Haar step on pairs of `n0 / 2` rows of `stride` interleaved blocks.
pub(super) fn haar1(x: &mut [f32], n0: usize, stride: usize) {
    let half = n0 >> 1;
    // QCONST16(.70710678f, 15) of the float build.
    let s = std::f32::consts::FRAC_1_SQRT_2;
    for i in 0..stride {
        for j in 0..half {
            let a = 2 * j * stride + i;
            let b = (2 * j + 1) * stride + i;
            if b >= x.len() {
                return;
            }
            let tmp1 = s * x[a];
            let tmp2 = s * x[b];
            x[a] = tmp1 + tmp2;
            x[b] = tmp1 - tmp2;
        }
    }
}

fn ordery(stride: usize) -> &'static [usize] {
    ORDERY_TABLE.get(stride.wrapping_sub(2)..).unwrap_or(&[])
}

/// `deinterleave_hadamard`.
fn deinterleave_hadamard(x: &mut [f32], n0: usize, stride: usize, hadamard: bool) {
    let n = n0 * stride;
    if n > x.len() || n > MAX_BAND {
        return;
    }
    let mut tmp = [0.0f32; MAX_BAND];
    if hadamard && stride >= 2 {
        let ord = ordery(stride);
        for i in 0..stride {
            for j in 0..n0 {
                tmp[ord[i] * n0 + j] = x[j * stride + i];
            }
        }
    } else {
        for i in 0..stride {
            for j in 0..n0 {
                tmp[i * n0 + j] = x[j * stride + i];
            }
        }
    }
    x[..n].copy_from_slice(&tmp[..n]);
}

/// `interleave_hadamard`.
fn interleave_hadamard(x: &mut [f32], n0: usize, stride: usize, hadamard: bool) {
    let n = n0 * stride;
    if n > x.len() || n > MAX_BAND {
        return;
    }
    let mut tmp = [0.0f32; MAX_BAND];
    if hadamard && stride >= 2 {
        let ord = ordery(stride);
        for i in 0..stride {
            for j in 0..n0 {
                tmp[j * stride + i] = x[ord[i] * n0 + j];
            }
        }
    } else {
        for i in 0..stride {
            for j in 0..n0 {
                tmp[j * stride + i] = x[i * n0 + j];
            }
        }
    }
    x[..n].copy_from_slice(&tmp[..n]);
}

/// `compute_qn`: the resolution of a split angle.
fn compute_qn(n: i32, b: i32, offset: i32, pulse_cap: i32, stereo: bool) -> i32 {
    const EXP2_TABLE8: [i32; 8] = [16384, 17866, 19483, 21247, 23170, 25267, 27554, 30048];
    let mut n2 = 2 * n - 1;
    if stereo && n == 2 {
        n2 -= 1;
    }
    let mut qb = (b - pulse_cap - (4 << BITRES)).min((b + n2 * offset) / n2);
    qb = qb.min(8 << BITRES);
    if qb < (1 << BITRES >> 1) {
        1
    } else {
        let qn = EXP2_TABLE8[(qb & 0x7) as usize] >> (14 - (qb >> BITRES));
        (qn + 1) >> 1 << 1
    }
}

/// `stereo_merge`: mid and side back to left and right.
fn stereo_merge(x: &mut [f32], y: &mut [f32], mid: f32) {
    let mut xp = 0.0f32;
    let mut side = 0.0f32;
    for (&a, &b) in x.iter().zip(y.iter()) {
        xp += a * b;
        side += b * b;
    }
    xp *= mid;
    let mid2 = mid;
    let el = mid2 * mid2 + side - 2.0 * xp;
    let er = mid2 * mid2 + side + 2.0 * xp;
    if er < 6e-4 || el < 6e-4 {
        y.copy_from_slice(x);
        return;
    }
    let lgain = 1.0 / el.sqrt();
    let rgain = 1.0 / er.sqrt();
    for (a, b) in x.iter_mut().zip(y.iter_mut()) {
        let l = mid * *a;
        let r = *b;
        *a = lgain * (l - r);
        *b = rgain * (l + r);
    }
}

/// The two halves of a split band: mid and side coefficients, their fold sources, and where
/// the mid leaves its result for later folding.
type SplitHalves<'a> = (
    &'a mut [f32],
    &'a mut [f32],
    Option<&'a mut [f32]>,
    Option<&'a mut [f32]>,
    Option<&'a mut [f32]>,
);

/// What a band's recursion shares: the range decoder, the band, the spread, the budget, the
/// noise seed.
struct Ctx<'e, 'a, 'b> {
    ec: &'e mut Ec<'a, 'b>,
    band: usize,
    spread: i32,
    intensity: usize,
    remaining_bits: i32,
    seed: u32,
}

/// `quant_band` (decoding). `x` (and `y` in stereo) is the band's `n` coefficients,
/// `lowband` a scratch copy of what to fold from, `lowband_out` where to leave this band's
/// result for later folding. Returns the collapse mask.
#[allow(clippy::too_many_arguments)]
fn quant_band(
    ctx: &mut Ctx<'_, '_, '_>,
    x: &mut [f32],
    mut y: Option<&mut [f32]>,
    mut n: usize,
    mut b: i32,
    mut blocks: usize,
    mut tf_change: i32,
    mut lowband: Option<&mut [f32]>,
    mut lm: i32,
    mut lowband_out: Option<&mut [f32]>,
    level: i32,
    gain: f32,
    mut fill: u32,
) -> u32 {
    let n0 = n;
    let mut n_b = n;
    let mut b0 = blocks;
    let mut time_divide = 0;
    let mut recombine = 0;
    let mut inv = false;
    let long_blocks = b0 == 1;
    let mut cm: u32 = 0;
    let i = ctx.band;

    n_b /= blocks.max(1);
    let mut n_b0 = n_b;
    let stereo = y.is_some();
    let mut split = stereo;

    if n == 1 {
        {
            let mut chans: [Option<&mut [f32]>; 2] = [Some(&mut *x), y.as_deref_mut()];
            for ch in chans.iter_mut().flatten() {
                let mut sign = 0;
                if ctx.remaining_bits >= 1 << BITRES {
                    sign = ctx.ec.bits(1);
                    ctx.remaining_bits -= 1 << BITRES;
                }
                ch[0] = if sign != 0 { -1.0 } else { 1.0 };
            }
        }
        if let Some(out) = lowband_out {
            out[0] = x[0];
        }
        return 1;
    }

    if !stereo && level == 0 {
        if tf_change > 0 {
            recombine = tf_change;
        }
        for k in 0..recombine {
            const BIT_INTERLEAVE: [u32; 16] = [0, 1, 1, 1, 2, 3, 3, 3, 2, 3, 3, 3, 2, 3, 3, 3];
            if let Some(lb) = lowband.as_deref_mut() {
                haar1(lb, n >> k, 1 << k);
            }
            fill = BIT_INTERLEAVE[(fill & 0xF) as usize]
                | BIT_INTERLEAVE[((fill >> 4) & 0xF) as usize] << 2;
        }
        blocks >>= recombine;
        n_b <<= recombine;
        while n_b & 1 == 0 && tf_change < 0 {
            if let Some(lb) = lowband.as_deref_mut() {
                haar1(lb, n_b, blocks);
            }
            fill |= fill << blocks;
            blocks <<= 1;
            n_b >>= 1;
            time_divide += 1;
            tf_change += 1;
        }
        b0 = blocks;
        n_b0 = n_b;
        if b0 > 1
            && let Some(lb) = lowband.as_deref_mut()
        {
            deinterleave_hadamard(lb, n_b >> recombine, b0 << recombine, long_blocks);
        }
    }

    if !stereo && lm != -1 && b > split_threshold(i, lm) && n > 2 {
        n >>= 1;
        split = true;
        lm -= 1;
        if blocks == 1 {
            fill = (fill & 1) | (fill << 1);
        }
        blocks = (blocks + 1) >> 1;
    }

    if split {
        let mut itheta: i32 = 0;
        let pulse_cap = i32::from(LOG_N[i]) + lm * (1 << BITRES);
        let offset = (pulse_cap >> 1)
            - if stereo && n == 2 {
                QTHETA_OFFSET_TWOPHASE
            } else {
                QTHETA_OFFSET
            };
        let mut qn = compute_qn(n as i32, b, offset, pulse_cap, stereo);
        if stereo && i >= ctx.intensity {
            qn = 1;
        }
        let tell = ctx.ec.tell_frac();
        if qn != 1 {
            if stereo && n > 2 {
                let p0 = 3;
                let x0 = qn / 2;
                let ft = p0 * (x0 + 1) + x0;
                let fs = ctx.ec.decode(ft as u32) as i32;
                let xv = if fs < (x0 + 1) * p0 {
                    fs / p0
                } else {
                    x0 + 1 + (fs - (x0 + 1) * p0)
                };
                let (fl, fh) = if xv <= x0 {
                    (p0 * xv, p0 * (xv + 1))
                } else {
                    ((xv - 1 - x0) + (x0 + 1) * p0, (xv - x0) + (x0 + 1) * p0)
                };
                ctx.ec.update(fl as u32, fh as u32, ft as u32);
                itheta = xv;
            } else if b0 > 1 || stereo {
                itheta = ctx.ec.uint((qn + 1) as u32) as i32;
            } else {
                let ft = ((qn >> 1) + 1) * ((qn >> 1) + 1);
                let fm = ctx.ec.decode(ft as u32) as i32;
                let (fs, fl);
                if fm < (((qn >> 1) * ((qn >> 1) + 1)) >> 1) {
                    itheta = (isqrt32(8 * fm as u32 + 1) as i32 - 1) >> 1;
                    fs = itheta + 1;
                    fl = (itheta * (itheta + 1)) >> 1;
                } else {
                    itheta = (2 * (qn + 1) - isqrt32(8 * (ft - fm - 1) as u32 + 1) as i32) >> 1;
                    fs = qn + 1 - itheta;
                    fl = ft - (((qn + 1 - itheta) * (qn + 2 - itheta)) >> 1);
                }
                ctx.ec.update(fl as u32, (fl + fs) as u32, ft as u32);
            }
            itheta = itheta * 16384 / qn;
        } else if stereo {
            inv = if b > 2 << BITRES && ctx.remaining_bits > 2 << BITRES {
                ctx.ec.bit_logp(2)
            } else {
                false
            };
            itheta = 0;
        }
        let qalloc = ctx.ec.tell_frac() - tell;
        b -= qalloc;

        let orig_fill = fill;
        let (imid, iside, mut delta);
        if itheta == 0 {
            imid = 32767;
            iside = 0;
            fill &= (1u32 << blocks) - 1;
            delta = -16384;
        } else if itheta == 16384 {
            imid = 0;
            iside = 32767;
            fill &= ((1u32 << blocks) - 1) << blocks;
            delta = 16384;
        } else {
            imid = bitexact_cos(itheta);
            iside = bitexact_cos(16384 - itheta);
            delta = frac_mul16(((n - 1) << 7) as i32, bitexact_log2tan(iside, imid));
        }
        let mid = (1.0 / 32768.0) * imid as f32;
        let side = (1.0 / 32768.0) * iside as f32;

        if n == 2 && stereo {
            let y = y.as_deref_mut().expect("stereo has y");
            let mut mbits = b;
            let mut sbits = 0;
            if itheta != 0 && itheta != 16384 {
                sbits = 1 << BITRES;
            }
            mbits -= sbits;
            let c = itheta > 8192;
            ctx.remaining_bits -= qalloc + sbits;
            let mut sign = 0;
            if sbits != 0 {
                sign = ctx.ec.bits(1) as i32;
            }
            let sign = (1 - 2 * sign) as f32;
            {
                let (x2, y2): (&mut [f32], &mut [f32]) = if c {
                    (&mut *y, &mut *x)
                } else {
                    (&mut *x, &mut *y)
                };
                cm = quant_band(
                    ctx,
                    x2,
                    None,
                    n,
                    mbits,
                    blocks,
                    tf_change,
                    lowband,
                    lm,
                    lowband_out.take(),
                    level,
                    gain,
                    orig_fill,
                );
                y2[0] = -sign * x2[1];
                y2[1] = sign * x2[0];
            }
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
            if b0 > 1 && !stereo && (itheta & 0x3fff) != 0 {
                if itheta > 8192 {
                    delta -= delta >> (4 - lm);
                } else {
                    delta = 0.min(delta + ((n as i32) << BITRES >> (5 - lm)));
                }
            }
            let mut mbits = 0.max(b.min((b - delta) / 2));
            let mut sbits = b - mbits;
            ctx.remaining_bits -= qalloc;

            let next_level = if stereo { level } else { level + 1 };
            let (xm, ys, lb_mid, lb_side, out_mid): SplitHalves<'_> = if stereo {
                (
                    &mut *x,
                    y.as_deref_mut().expect("stereo has y"),
                    lowband,
                    None,
                    lowband_out.take(),
                )
            } else {
                let (lo, hi) = x.split_at_mut(n);
                let (l1, l2) = match lowband {
                    Some(l) => {
                        let (a, b) = l.split_at_mut(n.min(l.len()));
                        (Some(a), Some(b))
                    }
                    None => (None, None),
                };
                (lo, hi, l1, l2, None)
            };
            let side_shift = if stereo { 0 } else { b0 >> 1 };
            let mid_gain = if stereo { 1.0 } else { gain * mid };
            let mut rebalance = ctx.remaining_bits;
            if mbits >= sbits {
                cm = quant_band(
                    ctx, xm, None, n, mbits, blocks, tf_change, lb_mid, lm, out_mid, next_level,
                    mid_gain, fill,
                );
                rebalance = mbits - (rebalance - ctx.remaining_bits);
                if rebalance > 3 << BITRES && itheta != 0 {
                    sbits += rebalance - (3 << BITRES);
                }
                cm |= quant_band(
                    ctx,
                    ys,
                    None,
                    n,
                    sbits,
                    blocks,
                    tf_change,
                    lb_side,
                    lm,
                    None,
                    next_level,
                    gain * side,
                    fill >> blocks,
                ) << side_shift;
            } else {
                cm = quant_band(
                    ctx,
                    ys,
                    None,
                    n,
                    sbits,
                    blocks,
                    tf_change,
                    lb_side,
                    lm,
                    None,
                    next_level,
                    gain * side,
                    fill >> blocks,
                ) << side_shift;
                rebalance = sbits - (rebalance - ctx.remaining_bits);
                if rebalance > 3 << BITRES && itheta != 16384 {
                    mbits += rebalance - (3 << BITRES);
                }
                cm |= quant_band(
                    ctx, xm, None, n, mbits, blocks, tf_change, lb_mid, lm, out_mid, next_level,
                    mid_gain, fill,
                );
            }
        }
        if stereo {
            if n != 2 {
                let y = y.as_deref_mut().expect("stereo has y");
                stereo_merge(&mut x[..n], &mut y[..n], mid);
            }
            if inv {
                for v in y.expect("stereo has y").iter_mut() {
                    *v = -*v;
                }
            }
            return cm;
        }
    } else {
        let mut q = bits2pulses(i, lm, b);
        let mut curr_bits = pulses2bits(i, lm, q);
        ctx.remaining_bits -= curr_bits;
        while ctx.remaining_bits < 0 && q > 0 {
            ctx.remaining_bits += curr_bits;
            q -= 1;
            curr_bits = pulses2bits(i, lm, q);
            ctx.remaining_bits -= curr_bits;
        }
        let x = &mut x[..n];
        if q != 0 {
            let k = get_pulses(q) as usize;
            cm = alg_unquant(x, k, ctx.spread, blocks, ctx.ec, gain);
        } else {
            let cm_mask = ((1u64 << blocks) - 1) as u32;
            fill &= cm_mask;
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

    if level == 0 {
        // Mono only: stereo returned above.
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
        if let Some(out) = lowband_out.take() {
            let scale = (n0 as f32).sqrt();
            for (o, &v) in out.iter_mut().zip(x[..n0].iter()) {
                *o = scale * v;
            }
        }
        cm &= ((1u64 << blocks) - 1) as u32;
    }
    cm
}

/// What [`quant_all_bands`] needs beyond the coefficients.
pub(super) struct AllBands<'a> {
    pub(super) start: usize,
    pub(super) end: usize,
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
}

/// `quant_all_bands` (decoding): every band's shape into `x` (and `y`), `n` coefficients per
/// channel; the collapse masks into `collapse_masks` (`band * c + channel`); `seed` is the
/// folding noise's LCG state.
pub(super) fn quant_all_bands(
    a: &AllBands<'_>,
    x: &mut [f32],
    mut y: Option<&mut [f32]>,
    collapse_masks: &mut [u8; 2 * NB_EBANDS],
    ec: &mut Ec<'_, '_>,
    seed: &mut u32,
) {
    let c = if y.is_some() { 2 } else { 1 };
    let m = 1usize << a.lm;
    let blocks = if a.short_blocks { m } else { 1 };
    let norm_len = m * eb(NB_EBANDS);
    let mut norm_buf = [0.0f32; 2 * 8 * 100];
    let (norm, norm2) = norm_buf.split_at_mut(norm_len.min(800));
    let norm2 = &mut norm2[..norm_len.min(800)];
    let mut lowband_scratch = [0.0f32; MAX_BAND];
    let mut lowband_offset = 0usize;
    let mut update_lowband = true;
    let mut dual_stereo = a.dual_stereo;
    let mut balance = a.balance;
    let mut ctx = Ctx {
        ec,
        band: 0,
        spread: a.spread,
        intensity: a.intensity,
        remaining_bits: 0,
        seed: *seed,
    };
    for i in a.start..a.end {
        let lo = m * eb(i);
        let n = m * eb(i + 1) - lo;
        let tell = ctx.ec.tell_frac();
        if i != a.start {
            balance -= tell;
        }
        ctx.remaining_bits = a.total_bits - tell - 1;
        ctx.band = i;
        let b = if i < a.coded_bands {
            let curr_balance = balance / 3.min((a.coded_bands - i) as i32);
            0.max(16383.min((ctx.remaining_bits + 1).min(a.pulses[i] + curr_balance)))
        } else {
            0
        };
        if (lo as isize - n as isize >= (m * eb(a.start)) as isize || i == a.start + 1)
            && (update_lowband || lowband_offset == 0)
        {
            lowband_offset = i;
        }
        if i == a.start + 1 {
            // RFC 8251 section 9: repeat part of the first band so the second can fold.
            let n1 = m * (eb(a.start + 1) - eb(a.start));
            let n2 = m * (eb(a.start + 2) - eb(a.start + 1));
            let offset = m * eb(a.start);
            if n2 > n1 && 2 * n1 >= n2 {
                let from = offset + 2 * n1 - n2;
                norm.copy_within(from..from + n2 - n1, offset + n1);
                if c == 2 {
                    norm2.copy_within(from..from + n2 - n1, offset + n1);
                }
            }
        }
        let tf_change = a.tf_res[i];
        let mut effective_lowband: Option<usize> = None;
        let (mut x_cm, mut y_cm);
        if lowband_offset != 0 && (a.spread != SPREAD_AGGRESSIVE || blocks > 1 || tf_change < 0) {
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
            x_cm = ((1u64 << blocks) - 1) as u32;
            y_cm = x_cm;
        }
        if dual_stereo && i == a.intensity {
            dual_stereo = false;
            for j in m * eb(a.start)..lo {
                norm[j] = 0.5 * (norm[j] + norm2[j]);
            }
        }
        let xb = &mut x[lo..lo + n];
        // The fold source is copied first: it may overlap this band's output (hybrid).
        let copy_lowband = |src: &[f32], scratch: &mut [f32; MAX_BAND]| -> bool {
            match effective_lowband {
                Some(e) if e + n <= src.len() => {
                    scratch[..n].copy_from_slice(&src[e..e + n]);
                    true
                }
                _ => false,
            }
        };
        if dual_stereo {
            let yb = &mut y.as_deref_mut().expect("dual stereo has y")[lo..lo + n];
            let has = copy_lowband(norm, &mut lowband_scratch);
            x_cm = quant_band(
                &mut ctx,
                xb,
                None,
                n,
                b / 2,
                blocks,
                tf_change,
                has.then_some(&mut lowband_scratch[..n]),
                a.lm as i32,
                Some(&mut norm[lo..lo + n]),
                0,
                1.0,
                x_cm,
            );
            let has = copy_lowband(norm2, &mut lowband_scratch);
            y_cm = quant_band(
                &mut ctx,
                yb,
                None,
                n,
                b / 2,
                blocks,
                tf_change,
                has.then_some(&mut lowband_scratch[..n]),
                a.lm as i32,
                Some(&mut norm2[lo..lo + n]),
                0,
                1.0,
                y_cm,
            );
        } else {
            let yb = y.as_deref_mut().map(|y| &mut y[lo..lo + n]);
            let has = copy_lowband(norm, &mut lowband_scratch);
            x_cm = quant_band(
                &mut ctx,
                xb,
                yb,
                n,
                b,
                blocks,
                tf_change,
                has.then_some(&mut lowband_scratch[..n]),
                a.lm as i32,
                Some(&mut norm[lo..lo + n]),
                0,
                1.0,
                x_cm | y_cm,
            );
            y_cm = x_cm;
        }
        collapse_masks[i * c] = x_cm as u8;
        collapse_masks[i * c + c - 1] = y_cm as u8;
        balance += a.pulses[i] + tell;
        update_lowband = b > (n as i32) << BITRES;
    }
    *seed = ctx.seed;
}

/// `anti_collapse`: fills the blocks of a transient frame that got no pulse with noise at an
/// energy derived from the previous frames'.
#[allow(clippy::too_many_arguments)]
pub(super) fn anti_collapse(
    x: &mut [f32],
    collapse_masks: &[u8; 2 * NB_EBANDS],
    lm: usize,
    c: usize,
    size: usize,
    start: usize,
    end: usize,
    log_e: &[f32; 2 * NB_EBANDS],
    prev1: &[f32; 2 * NB_EBANDS],
    prev2: &[f32; 2 * NB_EBANDS],
    pulses: &[i32; NB_EBANDS],
    mut seed: u32,
) {
    for i in start..end {
        let n0 = eb(i + 1) - eb(i);
        let depth = (1 + pulses[i]) / ((n0 as i32) << lm);
        let thresh = 0.5 * exp2(-0.125 * depth as f32);
        let sqrt_1 = 1.0 / (((n0 << lm) as f32).sqrt());
        for ch in 0..c {
            let mut p1 = prev1[ch * NB_EBANDS + i];
            let mut p2 = prev2[ch * NB_EBANDS + i];
            if c == 1 {
                p1 = p1.max(prev1[NB_EBANDS + i]);
                p2 = p2.max(prev2[NB_EBANDS + i]);
            }
            let ediff = (log_e[ch * NB_EBANDS + i] - p1.min(p2)).max(0.0);
            let mut r = 2.0 * exp2(-ediff);
            if lm == 3 {
                // 1.41421356f
                r *= std::f32::consts::SQRT_2;
            }
            r = thresh.min(r);
            r *= sqrt_1;
            let base = ch * size + (eb(i) << lm);
            let band = &mut x[base..base + (n0 << lm)];
            let mut renormalize = false;
            for k in 0..1usize << lm {
                if collapse_masks[i * c + ch] & (1 << k) == 0 {
                    for j in 0..n0 {
                        seed = lcg_rand(seed);
                        band[(j << lm) + k] = if seed & 0x8000 != 0 { r } else { -r };
                    }
                    renormalize = true;
                }
            }
            if renormalize {
                renormalise_vector(band, 1.0);
            }
        }
    }
}

/// `denormalise_bands`: shapes times amplitudes, zero above `end`; `freq` is one channel of
/// `n` coefficients.
pub(super) fn denormalise_bands(x: &[f32], freq: &mut [f32], band_e: &[f32], end: usize, m: usize) {
    let mut at = 0;
    for i in 0..end {
        let g = band_e[i];
        let hi = m * eb(i + 1);
        for (f, &v) in freq[at..hi].iter_mut().zip(&x[at..hi]) {
            *f = v * g;
        }
        at = hi;
    }
    freq[at..].fill(0.0);
}

#[cfg(test)]
#[path = "bands_tests.rs"]
mod tests;
