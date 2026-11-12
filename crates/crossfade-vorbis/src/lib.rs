//! Vorbis I decoding in pure Rust, depending only on `libm`.
#![forbid(unsafe_code)]

mod bits;
pub mod channels;
mod codebook;
mod decoder;
mod error;
mod floor;
pub mod header;
mod mdct;
mod residue;
mod setup;
#[cfg(any(test, feature = "writer"))]
pub mod writer;

pub use channels::{Speaker, speakers};
pub use decoder::Decoder;
pub use error::{Error, Result};
pub use header::{Comments, HeaderKind, Ident, header_kind};
pub use setup::{Modes, Setup, packet_frames};
