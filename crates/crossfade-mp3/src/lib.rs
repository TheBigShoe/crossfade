//! MPEG audio Layer III (MP3) in pure Rust, usable without the rest of Crossfade.
//!
//! [`header::FrameHeader`] parses the four-byte header of any MPEG-1, MPEG-2 or MPEG-2.5
//! audio frame (layers I-III, so a reader can name what it refuses); [`info::InfoFrame`] reads
//! the Xing, LAME and VBRI headers an encoder may put in the first frame; [`Decoder`] turns
//! whole Layer III frames into interleaved `f32` samples, the same bits on every platform.
//!
//! ```
//! use crossfade_mp3::{Decoder, FrameHeader};
//!
//! // One silent MPEG-1 frame: 128 kbit/s, 44.1 kHz, mono, no main data.
//! let mut frame = vec![0u8; 417];
//! frame[..4].copy_from_slice(&[0xFF, 0xFB, 0x90, 0xC4]);
//! let mut pcm = Vec::new();
//! let info = Decoder::new().decode_frame(&frame, &mut pcm)?;
//! assert_eq!((info.samples, info.header.channels()), (1152, 1));
//! assert_eq!(pcm.len(), 1152);
//! # Ok::<(), crossfade_mp3::Error>(())
//! ```
//!
//! The Huffman trees and the filterbank algorithms and constants follow minimp3 (public
//! domain, CC0 1.0) by way of its Rust port nanomp3-core (MIT OR Apache-2.0, Copyright (c)
//! 2025 Robert B. Langer); the rest is written from ISO/IEC 11172-3 and 13818-3.
#![forbid(unsafe_code)]

mod bits;
mod decoder;
mod error;
pub mod header;
mod huffman;
mod hybrid;
pub mod info;
mod lanes;
mod requantize;
mod scalefactors;
mod sideinfo;
mod stereo;
mod synth;
mod tables;

pub use decoder::{Decoder, FrameInfo, MAX_FRAME_LEN};
pub use error::Error;
pub use header::{ChannelMode, FrameHeader, Layer, Version};
pub use synth::OUTPUT_LIMIT;
