//! RIFF/WAVE container.

mod demux;

pub use demux::WavDemuxer;

/// Whole frames per packet; the last packet of a stream may be shorter.
pub const FRAMES_PER_PACKET: usize = 4096;

#[cfg(test)]
mod tests;
