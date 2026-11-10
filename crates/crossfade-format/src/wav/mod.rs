//! RIFF/WAVE container.

mod demux;
mod mux;

pub use demux::WavDemuxer;
pub use mux::WavMuxer;

/// Whole frames per packet; the last packet of a stream may be shorter.
pub const FRAMES_PER_PACKET: usize = 4096;

#[cfg(test)]
mod tests;
