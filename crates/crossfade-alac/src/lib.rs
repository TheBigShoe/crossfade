//! Apple Lossless (ALAC) decoding in pure Rust. No dependencies, no unsafe code.
#![forbid(unsafe_code)]

mod bits;
pub mod config;
mod decode;
mod error;
mod predictor;
mod rice;
#[cfg(test)]
mod testing;

pub use config::{Config, WaveLayout, wave_layout};
pub use decode::Decoder;
pub use error::{Error, Result};
