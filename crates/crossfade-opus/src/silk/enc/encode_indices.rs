//! A frame's side information, written (libopus `silk/encode_indices.c`; RFC 6716
//! 4.2.7.3-4.2.7.9 in the encoder's direction).

use crate::range_enc::RangeEncoder;
use crate::silk::nlsf::nlsf_unpack;
use crate::silk::state::{CondCoding, MAX_LPC_ORDER, SideInfoIndices, TYPE_VOICED};
use crate::silk::tables::{
    DELTA_GAIN_ICDF, GAIN_ICDF, LTP_GAIN_ICDF, LTP_PER_INDEX_ICDF, LTPSCALE_ICDF, NLSF_EXT_ICDF,
    NLSF_INTERPOLATION_FACTOR_ICDF, PITCH_DELTA_ICDF, PITCH_LAG_ICDF, TYPE_OFFSET_NO_VAD_ICDF,
    TYPE_OFFSET_VAD_ICDF, UNIFORM4_ICDF, UNIFORM8_ICDF,
};
use crate::silk::tables_nlsf::NlsfCodebook;

const NLSF_QUANT_MAX_AMPLITUDE: i32 = 4;

/// What `silk_encode_indices` reads of the encoder's state besides the indices, and the
/// conditional-coding memory it updates.
pub(super) struct IndexCoder<'a> {
    pub(super) nb_subfr: usize,
    pub(super) fs_khz: i32,
    pub(super) nlsf_cb: &'a NlsfCodebook,
    pub(super) pitch_lag_low_bits_icdf: &'a [u8],
    pub(super) pitch_contour_icdf: &'a [u8],
    pub(super) ec_prev_signal_type: &'a mut i32,
    pub(super) ec_prev_lag_index: &'a mut i16,
}

/// `silk_encode_indices`: `lbrr` codes an LBRR frame (always with voice activity).
pub(super) fn encode_indices(
    c: &mut IndexCoder<'_>,
    ix: &SideInfoIndices,
    enc: &mut RangeEncoder,
    lbrr: bool,
    cond_coding: CondCoding,
) {
    let type_offset = 2 * i32::from(ix.signal_type) + i32::from(ix.quant_offset_type);
    if lbrr || type_offset >= 2 {
        enc.enc_icdf((type_offset - 2) as usize, &TYPE_OFFSET_VAD_ICDF, 8);
    } else {
        enc.enc_icdf(type_offset as usize, &TYPE_OFFSET_NO_VAD_ICDF, 8);
    }

    if cond_coding == CondCoding::Conditionally {
        enc.enc_icdf(ix.gains_indices[0] as usize, &DELTA_GAIN_ICDF, 8);
    } else {
        enc.enc_icdf(
            (ix.gains_indices[0] >> 3) as usize,
            &GAIN_ICDF[ix.signal_type as usize],
            8,
        );
        enc.enc_icdf((ix.gains_indices[0] & 7) as usize, &UNIFORM8_ICDF, 8);
    }
    for i in 1..c.nb_subfr {
        enc.enc_icdf(ix.gains_indices[i] as usize, &DELTA_GAIN_ICDF, 8);
    }

    let cb = c.nlsf_cb;
    let first = (ix.signal_type as usize >> 1) * cb.n_vectors;
    enc.enc_icdf(ix.nlsf_indices[0] as usize, &cb.cb1_icdf[first..], 8);
    let mut ec_ix = [0i16; MAX_LPC_ORDER];
    let mut pred_q8 = [0u8; MAX_LPC_ORDER];
    nlsf_unpack(&mut ec_ix, &mut pred_q8, cb, ix.nlsf_indices[0] as usize);
    for i in 0..cb.order {
        let table = &cb.ec_icdf[ec_ix[i] as usize..];
        let v = i32::from(ix.nlsf_indices[i + 1]);
        if v >= NLSF_QUANT_MAX_AMPLITUDE {
            enc.enc_icdf(2 * NLSF_QUANT_MAX_AMPLITUDE as usize, table, 8);
            enc.enc_icdf((v - NLSF_QUANT_MAX_AMPLITUDE) as usize, &NLSF_EXT_ICDF, 8);
        } else if v <= -NLSF_QUANT_MAX_AMPLITUDE {
            enc.enc_icdf(0, table, 8);
            enc.enc_icdf((-v - NLSF_QUANT_MAX_AMPLITUDE) as usize, &NLSF_EXT_ICDF, 8);
        } else {
            enc.enc_icdf((v + NLSF_QUANT_MAX_AMPLITUDE) as usize, table, 8);
        }
    }
    if c.nb_subfr == 4 {
        enc.enc_icdf(
            ix.nlsf_interp_coef_q2 as usize,
            &NLSF_INTERPOLATION_FACTOR_ICDF,
            8,
        );
    }

    if i32::from(ix.signal_type) == TYPE_VOICED {
        let mut absolute = true;
        if cond_coding == CondCoding::Conditionally && *c.ec_prev_signal_type == TYPE_VOICED {
            let mut delta = i32::from(ix.lag_index) - i32::from(*c.ec_prev_lag_index);
            if !(-8..=11).contains(&delta) {
                delta = 0;
            } else {
                delta += 9;
                absolute = false;
            }
            enc.enc_icdf(delta as usize, &PITCH_DELTA_ICDF, 8);
        }
        if absolute {
            let half = c.fs_khz >> 1;
            let high = i32::from(ix.lag_index) / half;
            let low = i32::from(ix.lag_index) - high * half;
            enc.enc_icdf(high as usize, &PITCH_LAG_ICDF, 8);
            enc.enc_icdf(low as usize, c.pitch_lag_low_bits_icdf, 8);
        }
        *c.ec_prev_lag_index = ix.lag_index;
        enc.enc_icdf(ix.contour_index as usize, c.pitch_contour_icdf, 8);
        enc.enc_icdf(ix.per_index as usize, &LTP_PER_INDEX_ICDF, 8);
        for k in 0..c.nb_subfr {
            enc.enc_icdf(
                ix.ltp_index[k] as usize,
                LTP_GAIN_ICDF[ix.per_index as usize],
                8,
            );
        }
        if cond_coding == CondCoding::Independently {
            enc.enc_icdf(ix.ltp_scale_index as usize, &LTPSCALE_ICDF, 8);
        }
    }
    *c.ec_prev_signal_type = i32::from(ix.signal_type);
    enc.enc_icdf(ix.seed as usize, &UNIFORM4_ICDF, 8);
}

#[cfg(test)]
#[path = "encode_indices_tests.rs"]
mod tests;
