//! Shared vocabulary of the Crossfade engine: time, audio descriptions and
//! the I/O and clock seams every other engine crate builds on. Pure Rust,
//! no native dependencies.
#![forbid(unsafe_code)]

mod error;
mod rational;
mod time;

pub use error::Error;
pub use rational::Rational;
pub use time::{Rounding, TimeBase, Timestamp};
