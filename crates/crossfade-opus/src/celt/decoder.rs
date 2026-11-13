//! `celt_decode_with_ec` and `celt_decode_lost` (`celt/celt.c`, the decoder half): the frame's
//! symbols in the reference's order, synthesis through the IMDCT, the comb post-filter and
//! de-emphasis, and the concealment of lost frames.

use super::{CeltDecoder, CeltFrame};
use crate::error::Error;
use crate::range::RangeDecoder;
#[cfg(test)]
#[path = "decoder_tests.rs"]
mod tests;
