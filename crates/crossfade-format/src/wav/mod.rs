//! RIFF/WAVE container.

mod demux;
mod mux;

// The AIFF reader streams with the same primitives.
pub use demux::WavDemuxer;
pub(crate) use demux::{fill, fill_exact, skip};
pub use mux::WavMuxer;

/// Whole frames per packet; the last packet of a stream may be shorter.
pub const FRAMES_PER_PACKET: usize = 4096;

#[cfg(test)]
mod tag_tests;
#[cfg(test)]
mod tests;
