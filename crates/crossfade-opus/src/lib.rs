//! Opus (RFC 6716, updated by RFC 8251) in pure Rust, depending only on `libm`: the
//! [`Decoder`] (SILK, CELT, hybrid, multistream) and the [`Encoder`] (CELT-only for now).
//!
//! A port of the reference decoder and of libopus: see the crate's `COPYING` (3-clause BSD)
//! for the notice and the royalty-free patent licences that cover Opus.
#![forbid(unsafe_code)]

mod celt;
mod decoder;
mod encoder;
mod error;
pub mod head;
mod multistream;
mod multistream_enc;
pub mod packet;
pub mod range;
pub mod range_enc;
mod repacketizer;
mod silk;
#[doc(hidden)]
pub mod testing;

pub use decoder::Decoder;
pub use encoder::{Application, Encoder, EncoderConfig, FrameDuration, Signal};
pub use error::{Error, Result};
pub use head::{ChannelMapping, OpusHead, OpusTags};
pub use multistream::MultistreamDecoder;
pub use multistream_enc::MultistreamEncoder;
pub use packet::{Bandwidth, Mode, Packet, Toc, packet_duration};
pub use range::RangeDecoder;
pub use range_enc::RangeEncoder;
