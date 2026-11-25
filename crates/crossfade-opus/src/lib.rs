//! Opus (RFC 6716, updated by RFC 8251) in pure Rust, depending only on `libm`: the
//! [`Decoder`] and [`MultistreamDecoder`] (SILK, CELT, hybrid), the [`Encoder`] (SILK-only,
//! hybrid or CELT-only by the rate and the speech/music analysis or a [`Signal`] hint, as
//! libopus chooses; 2.5 to 60 ms; CBR and VBR; FEC and DTX) and the [`MultistreamEncoder`]
//! (mapping families 0 and 1, up to 7.1, an LFE stream in CELT's LFE mode, surround
//! masking), and the Ogg headers ([`OpusHead`], whose `to_bytes` an encoder's `head` feeds to a writer).
//!
//! A port of the reference decoder and of libopus: see the crate's `COPYING` (3-clause BSD)
//! for the notice and the royalty-free patent licences that cover Opus.
#![forbid(unsafe_code)]

mod analysis;
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
