//! FLAC reader.

use crossfade_core::{Demuxer, Error, Limits, MediaSource};

pub fn open(_source: Box<dyn MediaSource>, _limits: &Limits) -> Result<Box<dyn Demuxer>, Error> {
    Err(Error::Unsupported {
        what: "reading flac is not built yet".into(),
    })
}
