//! The excitation's pulses, written (libopus `silk/encode_pulses.c`, the encode halves of
//! `shell_coder.c` and `code_signs.c`; RFC 6716 4.2.7.8).

use super::tables::{MAX_PULSES_TABLE, PULSES_PER_BLOCK_BITS_Q5, RATE_LEVELS_BITS_Q5};
use crate::range_enc::RangeEncoder;
use crate::silk::state::{MAX_FRAME_LENGTH, SHELL_CODEC_FRAME_LENGTH};
use crate::silk::tables::{
    LSB_ICDF, PULSES_PER_BLOCK_ICDF, RATE_LEVELS_ICDF, SHELL_CODE_TABLE_OFFSETS, SHELL_CODE_TABLE0,
    SHELL_CODE_TABLE1, SHELL_CODE_TABLE2, SHELL_CODE_TABLE3, SIGN_ICDF,
};

const MAX_PULSES: usize = 16;
const N_RATE_LEVELS: usize = 10;
const MAX_NB_SHELL_BLOCKS: usize = MAX_FRAME_LENGTH / SHELL_CODEC_FRAME_LENGTH;

/// `combine_and_check`: pairwise sums into `out`, or `false` once one exceeds `max`.
fn combine_and_check(out: &mut [i32], input: &[i32], max: i32, len: usize) -> bool {
    for k in 0..len {
        let sum = input[2 * k] + input[2 * k + 1];
        if sum > max {
            return false;
        }
        out[k] = sum;
    }
    true
}

/// `silk_encode_pulses`: `pulses` holds the frame's signed pulses; a 10 ms frame at 12 kHz has
/// its last, partial, block zeroed and coded whole.
pub(super) fn encode_pulses(
    enc: &mut RangeEncoder,
    signal_type: i32,
    quant_offset_type: i32,
    pulses: &mut [i8; MAX_FRAME_LENGTH],
    frame_length: usize,
) {
    let mut iter = frame_length / SHELL_CODEC_FRAME_LENGTH;
    if iter * SHELL_CODEC_FRAME_LENGTH < frame_length {
        iter += 1;
        pulses[frame_length..iter * SHELL_CODEC_FRAME_LENGTH].fill(0);
    }
    let n = iter * SHELL_CODEC_FRAME_LENGTH;
    let mut abs_pulses = [0i32; MAX_FRAME_LENGTH];
    for (a, &p) in abs_pulses[..n].iter_mut().zip(pulses.iter()) {
        *a = i32::from(p).abs();
    }

    let mut sum_pulses = [0i32; MAX_NB_SHELL_BLOCKS];
    let mut n_rshifts = [0usize; MAX_NB_SHELL_BLOCKS];
    let mut comb = [0i32; 8];
    for i in 0..iter {
        let block = &mut abs_pulses[i * SHELL_CODEC_FRAME_LENGTH..][..SHELL_CODEC_FRAME_LENGTH];
        loop {
            let mut ok = combine_and_check(&mut comb, block, MAX_PULSES_TABLE[0].into(), 8);
            let c = comb;
            ok &= combine_and_check(&mut comb, &c, MAX_PULSES_TABLE[1].into(), 4);
            let c = comb;
            ok &= combine_and_check(&mut comb, &c, MAX_PULSES_TABLE[2].into(), 2);
            ok &= combine_and_check(&mut sum_pulses[i..], &comb, MAX_PULSES_TABLE[3].into(), 1);
            if ok {
                break;
            }
            n_rshifts[i] += 1;
            for a in block.iter_mut() {
                *a >>= 1;
            }
        }
    }

    // The rate level that codes the pulse counts in the fewest bits.
    let mut min_sum_bits_q5 = i32::MAX;
    let mut rate_level = 0;
    for k in 0..N_RATE_LEVELS - 1 {
        let n_bits = &PULSES_PER_BLOCK_BITS_Q5[k];
        let mut sum_bits_q5 = i32::from(RATE_LEVELS_BITS_Q5[(signal_type >> 1) as usize][k]);
        for i in 0..iter {
            sum_bits_q5 += i32::from(if n_rshifts[i] > 0 {
                n_bits[MAX_PULSES + 1]
            } else {
                n_bits[sum_pulses[i] as usize]
            });
        }
        if sum_bits_q5 < min_sum_bits_q5 {
            min_sum_bits_q5 = sum_bits_q5;
            rate_level = k;
        }
    }
    enc.enc_icdf(
        rate_level,
        &RATE_LEVELS_ICDF[(signal_type >> 1) as usize],
        8,
    );

    let cdf = &PULSES_PER_BLOCK_ICDF[rate_level];
    let escape = &PULSES_PER_BLOCK_ICDF[N_RATE_LEVELS - 1];
    for i in 0..iter {
        if n_rshifts[i] == 0 {
            enc.enc_icdf(sum_pulses[i] as usize, cdf, 8);
        } else {
            enc.enc_icdf(MAX_PULSES + 1, cdf, 8);
            for _ in 0..n_rshifts[i] - 1 {
                enc.enc_icdf(MAX_PULSES + 1, escape, 8);
            }
            enc.enc_icdf(sum_pulses[i] as usize, escape, 8);
        }
    }

    for i in 0..iter {
        if sum_pulses[i] > 0 {
            shell_encoder(enc, &abs_pulses[i * SHELL_CODEC_FRAME_LENGTH..]);
        }
    }

    for i in 0..iter {
        if n_rshifts[i] > 0 {
            let n_ls = n_rshifts[i] - 1;
            for &p in &pulses[i * SHELL_CODEC_FRAME_LENGTH..][..SHELL_CODEC_FRAME_LENGTH] {
                // The C's `(opus_int8)silk_abs(p)`: -128 stays -128.
                let abs_q = i32::from(p.wrapping_abs());
                for j in (1..=n_ls).rev() {
                    enc.enc_icdf(((abs_q >> j) & 1) as usize, &LSB_ICDF, 8);
                }
                enc.enc_icdf((abs_q & 1) as usize, &LSB_ICDF, 8);
            }
        }
    }

    encode_signs(
        enc,
        pulses,
        frame_length,
        signal_type,
        quant_offset_type,
        &sum_pulses,
    );
}

/// `encode_split`.
fn encode_split(enc: &mut RangeEncoder, child1: i32, p: i32, table: &[u8]) {
    if p > 0 {
        let offset = usize::from(SHELL_CODE_TABLE_OFFSETS[p as usize]);
        enc.enc_icdf(child1 as usize, &table[offset..], 8);
    }
}

/// `silk_shell_encoder`: one block of 16 pulse magnitudes, splitting depth first.
fn shell_encoder(enc: &mut RangeEncoder, p0: &[i32]) {
    let mut p1 = [0i32; 8];
    let mut p2 = [0i32; 4];
    let mut p3 = [0i32; 2];
    for k in 0..8 {
        p1[k] = p0[2 * k] + p0[2 * k + 1];
    }
    for k in 0..4 {
        p2[k] = p1[2 * k] + p1[2 * k + 1];
    }
    for k in 0..2 {
        p3[k] = p2[2 * k] + p2[2 * k + 1];
    }
    let p4 = p3[0] + p3[1];
    encode_split(enc, p3[0], p4, &SHELL_CODE_TABLE3);
    for j in 0..2 {
        encode_split(enc, p2[2 * j], p3[j], &SHELL_CODE_TABLE2);
        for k in 2 * j..2 * j + 2 {
            encode_split(enc, p1[2 * k], p2[k], &SHELL_CODE_TABLE1);
            for m in 2 * k..2 * k + 2 {
                encode_split(enc, p0[2 * m], p1[m], &SHELL_CODE_TABLE0);
            }
        }
    }
}

/// `silk_encode_signs`.
fn encode_signs(
    enc: &mut RangeEncoder,
    pulses: &[i8; MAX_FRAME_LENGTH],
    length: usize,
    signal_type: i32,
    quant_offset_type: i32,
    sum_pulses: &[i32; MAX_NB_SHELL_BLOCKS],
) {
    let base = (7 * (quant_offset_type + 2 * signal_type)) as usize;
    let blocks = ((length + SHELL_CODEC_FRAME_LENGTH / 2) / SHELL_CODEC_FRAME_LENGTH)
        .min(MAX_NB_SHELL_BLOCKS);
    for (i, block) in pulses
        .as_chunks::<SHELL_CODEC_FRAME_LENGTH>()
        .0
        .iter()
        .take(blocks)
        .enumerate()
    {
        let p = sum_pulses[i];
        if p > 0 {
            let icdf = [SIGN_ICDF[base + (p & 0x1F).min(6) as usize], 0];
            for &q in block {
                if q != 0 {
                    // silk_enc_map: negative -> 0, positive -> 1.
                    enc.enc_icdf(usize::from(q > 0), &icdf, 8);
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "encode_pulses_tests.rs"]
mod tests;
