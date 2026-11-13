//! A frame's side information (libopus `silk/decode_indices.c`, RFC 6716 4.2.7.3-4.2.7.9):
//! signal type and quantisation offset, gains, NLSF indices and interpolation factor, pitch
//! lag and contour, LTP filter and scaling, the seed.

use super::nlsf::nlsf_unpack;
use super::state::{ChannelState, CondCoding, MAX_LPC_ORDER, MAX_NB_SUBFR, TYPE_VOICED};
use super::tables::{
    DELTA_GAIN_ICDF, GAIN_ICDF, LTP_GAIN_ICDF, LTP_PER_INDEX_ICDF, LTPSCALE_ICDF, NLSF_EXT_ICDF,
    NLSF_INTERPOLATION_FACTOR_ICDF, PITCH_DELTA_ICDF, PITCH_LAG_ICDF, TYPE_OFFSET_NO_VAD_ICDF,
    TYPE_OFFSET_VAD_ICDF, UNIFORM4_ICDF, UNIFORM8_ICDF,
};
use crate::range::RangeDecoder;

const NLSF_QUANT_MAX_AMPLITUDE: i32 = 4;

/// `silk_decode_indices`.
pub(super) fn decode_indices(
    st: &mut ChannelState,
    dec: &mut RangeDecoder<'_>,
    frame_index: usize,
    decode_lbrr: bool,
    cond_coding: CondCoding,
) {
    let ix = if decode_lbrr || st.vad_flags[frame_index] {
        dec.dec_icdf(&TYPE_OFFSET_VAD_ICDF, 8) + 2
    } else {
        dec.dec_icdf(&TYPE_OFFSET_NO_VAD_ICDF, 8)
    };
    let signal_type = (ix >> 1) as i8;
    st.indices.signal_type = signal_type;
    st.indices.quant_offset_type = (ix & 1) as i8;

    if cond_coding == CondCoding::Conditionally {
        st.indices.gains_indices[0] = dec.dec_icdf(&DELTA_GAIN_ICDF, 8) as i8;
    } else {
        let msb = dec.dec_icdf(&GAIN_ICDF[signal_type as usize], 8) as i8;
        let lsb = dec.dec_icdf(&UNIFORM8_ICDF, 8) as i8;
        st.indices.gains_indices[0] = (msb << 3) + lsb;
    }
    for i in 1..st.nb_subfr.min(MAX_NB_SUBFR) {
        st.indices.gains_indices[i] = dec.dec_icdf(&DELTA_GAIN_ICDF, 8) as i8;
    }

    let cb = st.nlsf_cb;
    let first = (signal_type as usize >> 1) * cb.n_vectors;
    let cb1 = dec.dec_icdf(&cb.cb1_icdf[first..], 8);
    st.indices.nlsf_indices[0] = cb1 as i8;
    let mut ec_ix = [0i16; MAX_LPC_ORDER];
    let mut pred_q8 = [0u8; MAX_LPC_ORDER];
    nlsf_unpack(&mut ec_ix, &mut pred_q8, cb, cb1);
    for i in 0..cb.order {
        let mut ix = dec.dec_icdf(&cb.ec_icdf[ec_ix[i] as usize..], 8) as i32;
        if ix == 0 {
            ix -= dec.dec_icdf(&NLSF_EXT_ICDF, 8) as i32;
        } else if ix == 2 * NLSF_QUANT_MAX_AMPLITUDE {
            ix += dec.dec_icdf(&NLSF_EXT_ICDF, 8) as i32;
        }
        st.indices.nlsf_indices[i + 1] = (ix - NLSF_QUANT_MAX_AMPLITUDE) as i8;
    }

    st.indices.nlsf_interp_coef_q2 = if st.nb_subfr == MAX_NB_SUBFR {
        dec.dec_icdf(&NLSF_INTERPOLATION_FACTOR_ICDF, 8) as i8
    } else {
        4
    };

    if i32::from(signal_type) == TYPE_VOICED {
        let mut absolute = true;
        if cond_coding == CondCoding::Conditionally && st.ec_prev_signal_type == TYPE_VOICED {
            let delta = dec.dec_icdf(&PITCH_DELTA_ICDF, 8) as i32;
            if delta > 0 {
                // A hostile stream can walk the index anywhere; it wraps as the C's cast does.
                st.indices.lag_index = (i32::from(st.ec_prev_lag_index) + delta - 9) as i16;
                absolute = false;
            }
        }
        if absolute {
            let high = dec.dec_icdf(&PITCH_LAG_ICDF, 8) as i32 * (st.fs_khz >> 1);
            let low = dec.dec_icdf(st.pitch_lag_low_bits_icdf, 8) as i32;
            st.indices.lag_index = (high + low) as i16;
        }
        st.ec_prev_lag_index = st.indices.lag_index;
        st.indices.contour_index = dec.dec_icdf(st.pitch_contour_icdf, 8) as i8;

        let per = dec.dec_icdf(&LTP_PER_INDEX_ICDF, 8);
        st.indices.per_index = per as i8;
        for k in 0..st.nb_subfr.min(MAX_NB_SUBFR) {
            st.indices.ltp_index[k] = dec.dec_icdf(LTP_GAIN_ICDF[per], 8) as i8;
        }
        st.indices.ltp_scale_index = if cond_coding == CondCoding::Independently {
            dec.dec_icdf(&LTPSCALE_ICDF, 8) as i8
        } else {
            0
        };
    }
    st.ec_prev_signal_type = i32::from(signal_type);
    st.indices.seed = dec.dec_icdf(&UNIFORM4_ICDF, 8) as i8;
}
