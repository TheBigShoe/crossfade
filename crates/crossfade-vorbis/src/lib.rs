//! Vorbis I decoding in pure Rust, depending only on `libm`.
#![forbid(unsafe_code)]

mod bits;
mod codebook;
mod error;
mod floor;
mod mdct;
pub mod header;
mod residue;
mod setup;
#[cfg(any(test, feature = "writer"))]
pub mod writer;

pub use error::{Error, Result};
pub use header::{Comments, HeaderKind, Ident, header_kind};
pub use setup::{Modes, Setup, packet_frames};
