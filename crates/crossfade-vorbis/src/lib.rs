//! Vorbis I decoding (the Xiph.Org specification) in pure Rust, depending only on `libm`, so
//! that a stream decodes to the same bits on every platform, however its packets are batched.
//!
//! The crate takes packets; splitting an Ogg (or Matroska) file into them is a container
//! reader's job. A stream starts with three header packets: [`Ident`], [`Comments`] and
//! [`Setup`]. The setup is parsed against a byte budget, checked before anything is
//! allocated, and every index in it is checked once, there; a degenerate value that would make
//! a decoder divide by zero or loop is refused. [`Modes`] gives a packet's block size from its
//! first bits, and [`packet_frames`] what it yields, for a container's timestamps.
//!
//! ```no_run
//! use crossfade_vorbis::{Decoder, Ident, Setup};
//!
//! fn decode(headers: [&[u8]; 3], audio: &[&[u8]]) -> crossfade_vorbis::Result<Vec<f32>> {
//!     let ident = Ident::parse(headers[0])?;
//!     let setup = Setup::parse(headers[2], &ident, 64 << 20)?;
//!     let mut decoder = Decoder::new(&ident, setup)?;
//!     let mut pcm = Vec::new();
//!     for packet in audio {
//!         // A packet that cannot be decoded is an error for that packet alone.
//!         let _ = decoder.decode(packet, &mut pcm);
//!     }
//!     Ok(pcm) // interleaved, in Vorbis channel order (see `speakers`)
//! }
//! ```
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
#[doc(hidden)]
pub mod testing;
mod xiph;

pub use channels::{Speaker, speakers};
pub use decoder::Decoder;
#[cfg(feature = "profile")]
pub use decoder::STAGES;
pub use error::{Error, Result};
pub use header::{Comments, HeaderKind, Ident, header_kind};
pub use setup::{Modes, Setup, packet_frames};
pub use xiph::{pack_headers, unpack_headers};
