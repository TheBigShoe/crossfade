//! Vorbis I decoding in pure Rust, depending only on `libm`.
#![forbid(unsafe_code)]

mod bits;
pub mod channels;
mod codebook;
mod decoder;
mod error;
mod floor;
pub mod header;
#[cfg(test)]
mod hostile_tests;
mod mdct;
mod residue;
mod setup;
#[cfg(any(test, feature = "writer"))]
pub mod writer;
mod xiph;

pub use channels::{Speaker, speakers};
pub use decoder::Decoder;
#[cfg(feature = "profile")]
pub use decoder::STAGES;
pub use error::{Error, Result};
pub use header::{Comments, HeaderKind, Ident, header_kind};
pub use setup::{Modes, Setup, packet_frames};
pub use xiph::{pack_headers, unpack_headers};
