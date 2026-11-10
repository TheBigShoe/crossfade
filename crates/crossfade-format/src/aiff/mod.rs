//! AIFF and AIFF-C containers.

mod extended;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod writer_tests;

mod demux;
mod mux;
use crossfade_core::{Demuxer, Error, Limits, MediaSink, MediaSource, Muxer, StreamInfo, Tags};

pub use demux::AiffDemuxer;
pub use mux::AiffMuxer;

/// Whole frames per packet; the last packet of a stream may be shorter.
pub const FRAMES_PER_PACKET: usize = 4096;

/// Text chunks are tags, not payload: the reader skips larger ones and the writer omits them.
const TEXT_CAP: u64 = 64 * 1024;

pub fn open(source: Box<dyn MediaSource>, limits: &Limits) -> Result<Box<dyn Demuxer>, Error> {
    Ok(Box::new(AiffDemuxer::new(source, limits)?))
}

pub fn create(
    sink: Box<dyn MediaSink>,
    stream: &StreamInfo,
    tags: &Tags,
) -> Result<Box<dyn Muxer>, Error> {
    Ok(Box::new(AiffMuxer::new(sink, stream, tags)?))
}
