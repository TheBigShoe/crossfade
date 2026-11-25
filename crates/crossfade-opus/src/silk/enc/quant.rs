//! Gains and LTP filters, quantised (libopus `silk/gain_quant.c`'s encoder half,
//! `quant_LTP_gains.c`, `VQ_WMat_EC.c`).

use crate::silk::state::{LTP_ORDER, MAX_NB_SUBFR};
use crate::silk::tables::LTP_VQ_Q7;

#[cfg(test)]
#[path = "quant_tests.rs"]
mod tests;
