//! Vorbis I decoding in pure Rust, depending only on `libm`.
#![forbid(unsafe_code)]

mod bits;
mod codebook;
mod error;
pub mod header;
#[cfg(any(test, feature = "writer"))]
pub mod writer;

pub use error::{Error, Result};
pub use header::{Comments, HeaderKind, Ident, header_kind};
