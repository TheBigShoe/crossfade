//! Opus decoding (RFC 6716, updated by RFC 8251) in pure Rust, depending only on `libm`.
//!
//! A port of the reference decoder: see the crate's `COPYING` (3-clause BSD) for the notice
//! and the royalty-free patent licences that cover Opus.
#![forbid(unsafe_code)]

mod error;
pub mod range;
#[doc(hidden)]
pub mod testing;

pub use error::{Error, Result};
pub use range::RangeDecoder;
