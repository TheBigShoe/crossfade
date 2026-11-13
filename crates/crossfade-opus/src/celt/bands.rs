//! Band shapes (`celt/bands.c`, the decoding half): `quant_all_bands` and its recursive
//! `quant_band` (splitting, stereo angles, folding, the time-frequency transforms), the
//! anti-collapse noise and denormalisation. RFC 8251 section 9's hybrid folding is applied.

use super::ec::Ec;
use super::tables::NB_EBANDS;
#[cfg(test)]
#[path = "bands_tests.rs"]
mod tests;
