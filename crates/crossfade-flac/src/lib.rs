//! FLAC (RFC 9639) in pure Rust: frame and STREAMINFO parsing, a verifying decoder, and the
//! bit-level tools they share. No dependencies, no unsafe code.
//!
//! The decoder takes whole frames. Splitting a file into frames is a container reader's job;
//! [`frame::parse_header`] and [`crc`] are the pieces it needs.
//!
//! ```
//! use crossfade_flac::{Decoder, Verification};
//! use crossfade_flac::md5::Md5;
//! use crossfade_flac::streaminfo::StreamInfoBlock;
//!
//! // Two identical frames: 16 mono 16-bit samples, every one 0x1234 (a constant subframe).
//! const FRAME: [u8; 12] = [
//!     0xFF, 0xF8, 0x69, 0x08, 0x00, 0x0F, 0x30, 0x00, 0x12, 0x34, 0xAA, 0x3B,
//! ];
//! let mut md5 = Md5::new();
//! md5.update(&[0x34, 0x12].repeat(32));
//! let info = StreamInfoBlock {
//!     min_block_size: 16,
//!     max_block_size: 16,
//!     min_frame_size: 0,
//!     max_frame_size: 0,
//!     sample_rate: 44_100,
//!     channels: 1,
//!     bits_per_sample: 16,
//!     total_samples: 32,
//!     md5: md5.finalize(),
//! };
//!
//! let mut decoder = Decoder::new(&info)?;
//! let mut pcm = Vec::new();
//! for frame in [FRAME, FRAME] {
//!     let block = decoder.decode_frame(&frame)?;
//!     // Left-justified into 16 bits; `channel(0)` has the raw values instead.
//!     let mut samples = Vec::new();
//!     block.interleave_i16(16 - u32::from(block.bits_per_sample()), &mut samples);
//!     pcm.extend(samples);
//! }
//! assert_eq!(pcm, vec![0x1234; 32]);
//! assert_eq!(decoder.finish()?, Verification::Md5Matched);
//! # Ok::<(), crossfade_flac::Error>(())
//! ```
#![forbid(unsafe_code)]

pub mod bits;
pub mod crc;
pub mod decode;
pub mod encode;
pub mod error;
pub mod frame;
pub mod md5;
mod pool;
pub mod streaminfo;
mod subframe;

pub use decode::{Block, Decoder, Verification};
pub use encode::{Encoder, EncoderConfig};
pub use error::{Error, Result};
