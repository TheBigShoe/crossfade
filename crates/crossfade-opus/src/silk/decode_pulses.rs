//! The excitation's pulses (libopus `silk/decode_pulses.c`, `shell_coder.c`, `code_signs.c`;
//! RFC 6716 4.2.7.8): the rate level, the pulse count of each 16-sample block (with its
//! escape to extra LSBs), the shell coder's binary splits, the LSBs, the signs.

use super::state::{MAX_FRAME_LENGTH, SHELL_CODEC_FRAME_LENGTH};
use super::tables::{
    LSB_ICDF, PULSES_PER_BLOCK_ICDF, RATE_LEVELS_ICDF, SHELL_CODE_TABLE_OFFSETS, SHELL_CODE_TABLE0,
    SHELL_CODE_TABLE1, SHELL_CODE_TABLE2, SHELL_CODE_TABLE3, SIGN_ICDF,
};
use crate::range::RangeDecoder;

const MAX_PULSES: usize = 16;
const N_RATE_LEVELS: usize = 10;
const MAX_NB_SHELL_BLOCKS: usize = MAX_FRAME_LENGTH / SHELL_CODEC_FRAME_LENGTH;

/// `silk_decode_pulses`: `pulses` gets the frame's signed pulses (a 10 ms frame at 12 kHz
/// fills 128 entries: its last block is coded whole).
pub(super) fn decode_pulses(
    dec: &mut RangeDecoder<'_>,
    pulses: &mut [i16; MAX_FRAME_LENGTH],
    signal_type: i32,
    quant_offset_type: i32,
    frame_length: usize,
) {
    let rate_level = dec.dec_icdf(&RATE_LEVELS_ICDF[(signal_type >> 1) as usize], 8);
    let iter = frame_length
        .div_ceil(SHELL_CODEC_FRAME_LENGTH)
        .min(MAX_NB_SHELL_BLOCKS);

    let mut sum_pulses = [0usize; MAX_NB_SHELL_BLOCKS];
    let mut n_lshifts = [0usize; MAX_NB_SHELL_BLOCKS];
    for i in 0..iter {
        sum_pulses[i] = dec.dec_icdf(&PULSES_PER_BLOCK_ICDF[rate_level], 8);
        while sum_pulses[i] == MAX_PULSES + 1 {
            n_lshifts[i] += 1;
            // After ten LSBs the table is shifted so the escape cannot come again.
            let skip = usize::from(n_lshifts[i] == 10);
            sum_pulses[i] = dec.dec_icdf(&PULSES_PER_BLOCK_ICDF[N_RATE_LEVELS - 1][skip..], 8);
        }
    }

    for i in 0..iter {
        let block = &mut pulses[i * SHELL_CODEC_FRAME_LENGTH..][..SHELL_CODEC_FRAME_LENGTH];
        if sum_pulses[i] > 0 {
            shell_decoder(block, dec, sum_pulses[i]);
        } else {
            block.fill(0);
        }
    }

    for i in 0..iter {
        let n_ls = n_lshifts[i];
        if n_ls > 0 {
            let block = &mut pulses[i * SHELL_CODEC_FRAME_LENGTH..][..SHELL_CODEC_FRAME_LENGTH];
            for p in block.iter_mut() {
                let mut abs_q = i32::from(*p);
                for _ in 0..n_ls {
                    abs_q = (abs_q << 1) + dec.dec_icdf(&LSB_ICDF, 8) as i32;
                }
                // At most 16 << 10 plus ten LSBs: within 16 bits.
                *p = abs_q as i16;
            }
            sum_pulses[i] |= n_ls << 5;
        }
    }

    decode_signs(
        dec,
        pulses,
        frame_length,
        signal_type,
        quant_offset_type,
        &sum_pulses,
    );
}

/// `decode_split`: a parent's `p` pulses shared between two children.
fn decode_split(dec: &mut RangeDecoder<'_>, p: i16, table: &[u8]) -> (i16, i16) {
    if p > 0 {
        let offset = usize::from(SHELL_CODE_TABLE_OFFSETS[p as usize]);
        let child1 = dec.dec_icdf(&table[offset..], 8) as i16;
        (child1, p - child1)
    } else {
        (0, 0)
    }
}

/// `silk_shell_decoder`: one block of 16 pulses from its total, splitting depth first.
fn shell_decoder(out: &mut [i16], dec: &mut RangeDecoder<'_>, pulses4: usize) {
    let mut p3 = [0i16; 2];
    let mut p2 = [0i16; 4];
    let mut p1 = [0i16; 8];
    (p3[0], p3[1]) = decode_split(dec, pulses4 as i16, &SHELL_CODE_TABLE3);
    for j in 0..2 {
        (p2[2 * j], p2[2 * j + 1]) = decode_split(dec, p3[j], &SHELL_CODE_TABLE2);
        for k in 2 * j..2 * j + 2 {
            (p1[2 * k], p1[2 * k + 1]) = decode_split(dec, p2[k], &SHELL_CODE_TABLE1);
            for m in 2 * k..2 * k + 2 {
                (out[2 * m], out[2 * m + 1]) = decode_split(dec, p1[m], &SHELL_CODE_TABLE0);
            }
        }
    }
}

/// `silk_decode_signs`.
fn decode_signs(
    dec: &mut RangeDecoder<'_>,
    pulses: &mut [i16; MAX_FRAME_LENGTH],
    length: usize,
    signal_type: i32,
    quant_offset_type: i32,
    sum_pulses: &[usize; MAX_NB_SHELL_BLOCKS],
) {
    let base = (7 * (quant_offset_type + 2 * signal_type)) as usize;
    let blocks = ((length + SHELL_CODEC_FRAME_LENGTH / 2) / SHELL_CODEC_FRAME_LENGTH)
        .min(MAX_NB_SHELL_BLOCKS);
    for (i, block) in pulses
        .as_chunks_mut::<SHELL_CODEC_FRAME_LENGTH>()
        .0
        .iter_mut()
        .take(blocks)
        .enumerate()
    {
        let p = sum_pulses[i];
        if p > 0 {
            let icdf = [SIGN_ICDF[base + (p & 0x1F).min(6)], 0];
            for q in block.iter_mut() {
                if *q > 0 {
                    // silk_dec_map: 0 -> -1, 1 -> +1.
                    let sign = (dec.dec_icdf(&icdf, 8) as i16) * 2 - 1;
                    *q *= sign;
                }
            }
        }
    }
}
