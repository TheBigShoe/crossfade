#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

mod bits;
pub mod config;
mod decode;
mod encode;
mod error;
mod predictor;
mod rice;
mod write;

pub use config::{Config, WaveLayout, wave_layout};
pub use decode::Decoder;
pub use encode::Encoder;
pub use error::{Error, Result};
