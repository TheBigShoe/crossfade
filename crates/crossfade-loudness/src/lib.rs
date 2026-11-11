//! Programme loudness and peak measurement (ITU-R BS.1770-4, EBU R 128).
#![forbid(unsafe_code)]

mod error;
mod kweight;

/// Most channels a meter accepts.
pub const MAX_CHANNELS: usize = 64;

pub use error::{Error, Result};
pub use kweight::{Biquad, KWeighting, MAX_SAMPLE_RATE, MIN_SAMPLE_RATE};
