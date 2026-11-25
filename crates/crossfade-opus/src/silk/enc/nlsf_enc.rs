//! LPC coefficients to NLSFs and the NLSF quantiser (libopus `silk/A2NLSF.c`,
//! `NLSF_encode.c`, `NLSF_VQ.c`, `NLSF_del_dec_quant.c`, `NLSF_VQ_weights_laroia.c`,
//! `interpolate.c`; RFC 6716 4.2.7.5 read backwards).

use crate::silk::nlsf::nlsf_decode;
use crate::silk::state::MAX_LPC_ORDER;
use crate::silk::tables_nlsf::NlsfCodebook;

pub(super) fn a2nlsf(_nlsf: &mut [i16], _a_q16: &mut [i32]) {}

pub(super) fn nlsf_vq_weights_laroia(_w: &mut [i16], _nlsf: &[i16]) {}

pub(super) fn interpolate(_xi: &mut [i16], _x0: &[i16], _x1: &[i16], _ifact_q2: i32) {}

pub(super) fn nlsf_encode(
    _nlsf_indices: &mut [i8; MAX_LPC_ORDER + 1],
    _nlsf_q15: &mut [i16; MAX_LPC_ORDER],
    _cb: &NlsfCodebook,
    _w_q2: &[i16],
    _nlsf_mu_q20: i32,
    _n_survivors: usize,
    _signal_type: i32,
) -> i32 {
    0
}

#[cfg(test)]
#[path = "nlsf_enc_tests.rs"]
mod tests;
