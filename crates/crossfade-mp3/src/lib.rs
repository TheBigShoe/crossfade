//! MPEG audio Layer III (MP3) in pure Rust, usable without the rest of Crossfade.
//!
//! [`header::FrameHeader`] parses the four-byte header of any MPEG-1, MPEG-2 or MPEG-2.5
//! audio frame (layers I-III, so a reader can name what it refuses); [`info::InfoFrame`] reads
//! the Xing, LAME and VBRI headers an encoder may put in the first frame.
#![forbid(unsafe_code)]

mod error;
pub mod header;
pub mod info;

pub use error::Error;
pub use header::{ChannelMode, FrameHeader, Layer, Version};
