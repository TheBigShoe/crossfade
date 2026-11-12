//! Apple Lossless (ALAC) decoding in pure Rust. No dependencies, no unsafe code.
#![forbid(unsafe_code)]

pub mod config;
mod error;

pub use config::{Config, WaveLayout, wave_layout};
pub use error::{Error, Result};
