//! ISO base media files (`.mp4`, `.m4a`, `.m4b`). Not built yet.

mod asc;

use crossfade_core::{Demuxer, Error, Limits, MediaSource};

/// Opens an MP4 file; `source` starts at its first byte.
pub fn open(source: Box<dyn MediaSource>, limits: &Limits) -> Result<Box<dyn Demuxer>, Error> {
    let _ = (source, limits);
    Err(Error::Unsupported {
        what: "reading mp4 is not built yet".into(),
    })
}

#[cfg(test)]
mod tests;
