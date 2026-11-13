//! Band energy decoding (`celt/quant_bands.c`): coarse (Laplace, predicted across time and
//! frequency), fine and final bits, and the log to linear conversion with RFC 8251's cap.
//! Energies are log2 amplitudes, one row of [`NB_EBANDS`] per channel.

use super::ec::Ec;
use super::maths::exp2;
use super::tables::{BETA_COEF, BETA_INTRA, E_MEANS, E_PROB_MODEL, NB_EBANDS, PRED_COEF};
#[cfg(test)]
#[path = "quant_bands_tests.rs"]
mod tests;
