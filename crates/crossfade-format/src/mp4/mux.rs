//! The writer.

use crossfade_core::{Error, MediaSink, Muxer, StreamInfo, Tags};

/// An MP4 writer for `stream`.
pub fn create(
    _sink: Box<dyn MediaSink>,
    stream: &StreamInfo,
    _tags: &Tags,
) -> Result<Box<dyn Muxer>, Error> {
    Err(Error::Unsupported {
        what: format!(
            "writing {} into mp4 is not supported yet",
            stream.codec.name()
        ),
    })
}
