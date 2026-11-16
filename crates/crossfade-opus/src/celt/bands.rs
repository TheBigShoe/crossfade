//! Band shapes (`celt/bands.c`): `quant_all_bands` and its recursive `quant_band` (splitting,
//! stereo angles, folding, the time-frequency transforms) for either range coder, the
//! anti-collapse noise, and the conversions between spectrum and unit-norm bands. RFC 8251
//! section 9's hybrid folding is applied. The encoder always resynthesises: its shapes come
//! back as the decoder will have them.
#![allow(clippy::needless_range_loop)] // index loops as the reference writes them

use super::ec::Coder;
use super::maths::{bitexact_cos, bitexact_log2tan, exp2, fast_atan2f, frac_mul16, lcg_rand};
use super::rate::{BITRES, bits2pulses, get_pulses, pulses2bits, split_threshold};
use super::tables::{EBANDS, LOG_N, NB_EBANDS, ORDERY_TABLE};
use super::vq::{SPREAD_AGGRESSIVE, renormalise_vector};

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

/// `stereo_itheta`: the angle between two shapes in units of 1/16384 of a right angle, from
/// their energies (mid and side of left and right in stereo, the two halves otherwise).
fn stereo_itheta(x: &[f32], y: &[f32], stereo: bool) -> i32 {
    let mut emid = 1e-15f32;
    let mut eside = 1e-15f32;
    if stereo {
        for (&a, &b) in x.iter().zip(y.iter()) {
            let m = a + b;
            let s = a - b;
            emid += m * m;
            eside += s * s;
        }
    } else {
        for &a in x.iter() {
            emid += a * a;
        }
        for &b in y.iter() {
            eside += b * b;
        }
    }
    let mid = emid.sqrt();
    let side = eside.sqrt();
    // The reference's 0.63662f for 2 / pi, kept to the digit.
    #[allow(clippy::approx_constant)]
    let two_over_pi = 0.63662f32;
    (0.5 + 16384.0 * two_over_pi * fast_atan2f(side, mid)).floor() as i32
}

/// `intensity_stereo`: the two channels of a band as one shape, weighted by their energies.
fn intensity_stereo(x: &mut [f32], y: &[f32], left: f32, right: f32) {
    let norm = 1e-15 + (1e-15 + left * left + right * right).sqrt();
    let a1 = left / norm;
    let a2 = right / norm;
    for (l, &r) in x.iter_mut().zip(y.iter()) {
        *l = a1 * *l + a2 * r;
    }
}

/// `stereo_split`: left and right to mid and side.
fn stereo_split(x: &mut [f32], y: &mut [f32]) {
    let s = std::f32::consts::FRAC_1_SQRT_2;
    for (a, b) in x.iter_mut().zip(y.iter_mut()) {
        let l = s * *a;
        let r = s * *b;
        *a = l + r;
        *b = r - l;
    }
}

/// What a band's recursion shares: the range coder, the band, the spread, the budget, the
/// noise seed, and what only the encoder uses to choose its angles.
struct Ctx<'e, C: Coder> {
    ec: &'e mut C,
    band: usize,
    spread: i32,
    intensity: usize,
    remaining_bits: i32,
    seed: u32,
    /// The band amplitudes, a row per channel: intensity stereo weighs the channels by them.
    band_e: &'e [f32; 2 * NB_EBANDS],
    /// Round a stereo angle down (-1) or up (1) instead of to nearest (0).
    theta_round: i32,
    disable_inv: bool,
    /// Keep a split of the first band of a transient from leaving one side to noise.
    avoid_split_noise: bool,
}

/// `quant_band`, with `quant_partition` and `quant_band_stereo` in it. `x` (and `y` in
/// stereo) is the band's `n` coefficients: the shape to code for the encoder, and for both
/// the decoded shape on return. `lowband` is a scratch copy of what to fold from,
/// `lowband_out` where to leave this band's result for later folding. Returns the collapse
/// mask.
#[allow(clippy::too_many_arguments)]
fn quant_band<C: Coder>(
    ctx: &mut Ctx<'_, C>,
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
                    sign = ctx.ec.code_bits(u32::from(ch[0] < 0.0), 1);
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
            if C::ENCODE {
                haar1(x, n >> k, 1 << k);
            }
            if let Some(lb) = lowband.as_deref_mut() {
                haar1(lb, n >> k, 1 << k);
            }
            fill = BIT_INTERLEAVE[(fill & 0xF) as usize]
                | BIT_INTERLEAVE[((fill >> 4) & 0xF) as usize] << 2;
        }
        blocks >>= recombine;
        n_b <<= recombine;
        while n_b & 1 == 0 && tf_change < 0 {
            if C::ENCODE {
                haar1(x, n_b, blocks);
            }
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
        if b0 > 1 {
            if C::ENCODE {
                deinterleave_hadamard(x, n_b >> recombine, b0 << recombine, long_blocks);
            }
            if let Some(lb) = lowband.as_deref_mut() {
                deinterleave_hadamard(lb, n_b >> recombine, b0 << recombine, long_blocks);
            }
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
        if C::ENCODE {
            // The angle between mid and side (or the two halves): with it both can be
            // rescaled, since they have unit norm and are orthogonal.
            itheta = match y.as_deref() {
                Some(y) => stereo_itheta(&x[..n], &y[..n], true),
                None => stereo_itheta(&x[..n], &x[n..2 * n], false),
            };
        }
        let tell = ctx.ec.tell_frac();
        if qn != 1 {
            if C::ENCODE {
                if !stereo || ctx.theta_round == 0 {
                    itheta = (itheta * qn + 8192) >> 14;
                    if !stereo && ctx.avoid_split_noise && itheta > 0 && itheta < qn {
                        // Would this angle make the allocation leave one side to noise?
                        // Then that side gets no energy at all.
                        let unquantized = itheta * 16384 / qn;
                        let imid = bitexact_cos(unquantized);
                        let iside = bitexact_cos(16384 - unquantized);
                        let delta =
                            frac_mul16(((n - 1) << 7) as i32, bitexact_log2tan(iside, imid));
                        if delta > b {
                            itheta = qn;
                        } else if delta < -b {
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
            }
            itheta = if stereo && n > 2 {
                ctx.ec.code_theta_step(itheta, qn)
            } else if b0 > 1 || stereo {
                ctx.ec.code_uint(itheta as u32, (qn + 1) as u32) as i32
            } else {
                ctx.ec.code_theta_triangle(itheta, qn)
            };
            itheta = itheta * 16384 / qn;
            if C::ENCODE
                && let Some(y) = y.as_deref_mut()
            {
                if itheta == 0 {
                    let (l, r) = (ctx.band_e[i], ctx.band_e[NB_EBANDS + i]);
                    intensity_stereo(&mut x[..n], &y[..n], l, r);
                } else {
                    stereo_split(&mut x[..n], &mut y[..n]);
                }
            }
        } else if stereo {
            if C::ENCODE {
                let y = y.as_deref_mut().expect("stereo has y");
                inv = itheta > 8192 && !ctx.disable_inv;
                if inv {
                    for v in y[..n].iter_mut() {
                        *v = -*v;
                    }
                }
                let (l, r) = (ctx.band_e[i], ctx.band_e[NB_EBANDS + i]);
                intensity_stereo(&mut x[..n], &y[..n], l, r);
            }
            inv = if b > 2 << BITRES && ctx.remaining_bits > 2 << BITRES {
                ctx.ec.code_bit_logp(inv, 2)
            } else {
                false
            };
            // The flag is overridden so that a mono downmix is safe.
            if ctx.disable_inv {
                inv = false;
            }
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
            {
                let (x2, y2): (&mut [f32], &mut [f32]) = if c {
                    (&mut *y, &mut *x)
                } else {
                    (&mut *x, &mut *y)
                };
                let mut sign = 0;
                if sbits != 0 {
                    // Mid and side are orthogonal: the side is the mid turned, one way or
                    // the other.
                    let turned = x2[0] * y2[1] - x2[1] * y2[0] < 0.0;
                    sign = ctx.ec.code_bits(u32::from(turned), 1) as i32;
                }
                let sign = (1 - 2 * sign) as f32;
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
            cm = ctx.ec.code_pvq(x, k, ctx.spread, blocks, gain);
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
    /// The band amplitudes, a row per channel (the encoder's; the decoder's are not read).
    pub(super) band_e: &'a [f32; 2 * NB_EBANDS],
    /// Never code the intensity stereo phase inversion (the encoder's choice).
    pub(super) disable_inv: bool,
    /// Code each stereo band's angle rounded down and up and keep the nearer shape (the
    /// encoder at its highest complexities).
    pub(super) theta_rdo: bool,
}

/// `quant_all_bands`: every band's shape, `n` coefficients per channel in `x` (and `y`):
/// read into them by the decoder; coded from them by the encoder, which leaves there what the
/// decoder will read. The collapse masks go to `collapse_masks` (`band * c + channel`);
/// `seed` is the folding noise's LCG state.
pub(super) fn quant_all_bands<C: Coder>(
    a: &AllBands<'_>,
    x: &mut [f32],
    mut y: Option<&mut [f32]>,
    collapse_masks: &mut [u8; 2 * NB_EBANDS],
    ec: &mut C,
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
        band_e: a.band_e,
        theta_round: 0,
        disable_inv: a.disable_inv,
        // No noise in the first band of a transient; after it there is folding.
        avoid_split_noise: blocks > 1,
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
        // RFC 8251 section 9: repeat part of the first band so the second can fold.
        let hybrid_fold = |norm: &mut [f32], norm2: &mut [f32]| {
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
        };
        if i == a.start + 1 {
            hybrid_fold(norm, norm2);
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
        } else if let Some(yb) = y
            .as_deref_mut()
            .filter(|_| C::ENCODE && a.theta_rdo && i < a.intensity)
        {
            let yb = &mut yb[lo..lo + n];
            let cm = x_cm | y_cm;
            // Left and right weigh by their energies, the quieter one a little more.
            let (el, er) = (a.band_e[i], a.band_e[NB_EBANDS + i]);
            let min_e = el.min(er);
            let (wl, wr) = (el + min_e / 3.0, er + min_e / 3.0);
            let mut x_save = [0.0f32; MAX_BAND];
            let mut y_save = [0.0f32; MAX_BAND];
            x_save[..n].copy_from_slice(xb);
            y_save[..n].copy_from_slice(yb);
            let mut ec_save = Some(ctx.ec.save_state());
            let (bits_save, seed_save) = (ctx.remaining_bits, ctx.seed);
            let mut likeness = [0.0f32; 2];
            let mut first = None;
            for (k, round) in [-1, 1].into_iter().enumerate() {
                if k == 1 {
                    // Keep the first coding aside and start again from before it.
                    let mut kept = ([0.0f32; MAX_BAND], [0.0f32; MAX_BAND], [0.0f32; MAX_BAND]);
                    kept.0[..n].copy_from_slice(xb);
                    kept.1[..n].copy_from_slice(yb);
                    kept.2[..n].copy_from_slice(&norm[lo..lo + n]);
                    let state = ctx.ec.save_state();
                    first = Some((kept, state, ctx.remaining_bits, ctx.seed, x_cm));
                    xb.copy_from_slice(&x_save[..n]);
                    yb.copy_from_slice(&y_save[..n]);
                    if let Some(saved) = ec_save.take() {
                        ctx.ec.restore_state(saved);
                    }
                    ctx.remaining_bits = bits_save;
                    ctx.seed = seed_save;
                    // The first coding left its shape where the second band folds from.
                    if i == a.start + 1 {
                        hybrid_fold(norm, norm2);
                    }
                }
                ctx.theta_round = round;
                let has = copy_lowband(norm, &mut lowband_scratch);
                x_cm = quant_band(
                    &mut ctx,
                    xb,
                    Some(&mut *yb),
                    n,
                    b,
                    blocks,
                    tf_change,
                    has.then_some(&mut lowband_scratch[..n]),
                    a.lm as i32,
                    Some(&mut norm[lo..lo + n]),
                    0,
                    1.0,
                    cm,
                );
                let dot = |p: &[f32], q: &[f32]| p.iter().zip(q).map(|(u, v)| u * v).sum::<f32>();
                likeness[k] = wl * dot(&x_save[..n], xb) + wr * dot(&y_save[..n], yb);
            }
            ctx.theta_round = 0;
            if likeness[0] >= likeness[1]
                && let Some((kept, state, bits, seed, cm0)) = first
            {
                xb.copy_from_slice(&kept.0[..n]);
                yb.copy_from_slice(&kept.1[..n]);
                norm[lo..lo + n].copy_from_slice(&kept.2[..n]);
                ctx.ec.restore_state(state);
                ctx.remaining_bits = bits;
                ctx.seed = seed;
                x_cm = cm0;
            }
            y_cm = x_cm;
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
        ctx.avoid_split_noise = false;
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
