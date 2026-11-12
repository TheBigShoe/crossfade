//! RIFF/WAVE container.

mod demux;
mod mux;

// The AIFF reader streams with the same primitives.
pub use demux::WavDemuxer;
pub(crate) use demux::{fill, fill_exact, origin, seek_to, skip, source_len};
pub use mux::WavMuxer;

/// Whole frames per packet; the last packet of a stream may be shorter.
pub const FRAMES_PER_PACKET: usize = 4096;

/// A `LIST`/`INFO` chunk is tags, not payload: the reader skips a larger one, the writer
/// leaves out what would not fit.
const INFO_CAP: u64 = 1 << 20;

#[cfg(test)]
mod tag_tests;
#[cfg(test)]
mod tests;
