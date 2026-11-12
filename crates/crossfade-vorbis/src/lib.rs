//! Vorbis I decoding in pure Rust, depending only on `libm`.
#![forbid(unsafe_code)]

mod bits;
mod error;
pub mod header;

pub use error::{Error, Result};
pub use header::{Comments, HeaderKind, Ident, header_kind};
