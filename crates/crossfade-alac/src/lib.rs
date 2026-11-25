#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

mod bits;
pub mod config;
mod decode;
mod encode;
mod error;
mod predictor;
mod rice;
#[cfg(test)]
mod testing;

pub use config::{Config, WaveLayout, wave_layout};
pub use decode::Decoder;
pub use encode::{DEFAULT_FRAME_LENGTH, Encoder, EncoderConfig};
pub use error::{Error, Result};
