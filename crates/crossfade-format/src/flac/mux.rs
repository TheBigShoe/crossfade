//! FLAC writer.

use crossfade_core::{Error, MediaSink, Muxer, StreamInfo, Tags};

pub fn create(
    _sink: Box<dyn MediaSink>,
    _stream: &StreamInfo,
    _tags: &Tags,
) -> Result<Box<dyn Muxer>, Error> {
    Err(Error::Unsupported {
        what: "writing flac is not built yet".into(),
    })
}

#[cfg(test)]
mod tests;
