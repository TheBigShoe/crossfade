//! AIFF and AIFF-C containers.

mod extended;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod writer_tests;

mod demux;
use crossfade_core::{Demuxer, Error, Limits, MediaSink, MediaSource, Muxer, StreamInfo, Tags};

pub use demux::AiffDemuxer;

/// Whole frames per packet; the last packet of a stream may be shorter.
pub const FRAMES_PER_PACKET: usize = 4096;

pub fn open(source: Box<dyn MediaSource>, limits: &Limits) -> Result<Box<dyn Demuxer>, Error> {
    Ok(Box::new(AiffDemuxer::new(source, limits)?))
}

pub fn create(
    _sink: Box<dyn MediaSink>,
    _stream: &StreamInfo,
    _tags: &Tags,
) -> Result<Box<dyn Muxer>, Error> {
    Err(Error::Unsupported {
        what: "writing aiff is not built yet".into(),
    })
}
