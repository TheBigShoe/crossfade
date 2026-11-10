//! AIFF and AIFF-C containers.

mod extended;

use crossfade_core::{Demuxer, Error, Limits, MediaSink, MediaSource, Muxer, StreamInfo, Tags};

pub fn open(_source: Box<dyn MediaSource>, _limits: &Limits) -> Result<Box<dyn Demuxer>, Error> {
    Err(Error::Unsupported {
        what: "reading aiff is not built yet".into(),
    })
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
