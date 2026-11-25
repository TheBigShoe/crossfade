//! The SILK encoder (libopus `silk/enc_API.c` and everything under it, the fixed-point build
//! `silk/fixed/`): a port, its modules named after the C files they port.

// Nothing in the crate calls the encoder yet.
#![cfg_attr(not(test), allow(dead_code))]

mod encode_indices;
mod encode_pulses;
mod fix;
mod nlsf_enc;
mod pitch;
mod quant;
mod tables;

use crate::range_enc::RangeEncoder;

#[cfg(test)]
#[path = "enc_tests.rs"]
mod tests;
