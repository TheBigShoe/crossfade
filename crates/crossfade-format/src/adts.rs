//! AAC in ADTS frames (`.aac`). Not built yet.

use crossfade_core::{Demuxer, Error, Limits, MediaSource};

/// Opens an ADTS stream. `source` is positioned at the first frame: ID3v2 tags in front of it
/// were consumed while sniffing (and are dropped), and positions on `source` are offsets in the
/// whole input.
pub fn open(source: Box<dyn MediaSource>, limits: &Limits) -> Result<Box<dyn Demuxer>, Error> {
    let _ = (source, limits);
    Err(Error::Unsupported {
        what: "reading adts is not built yet".into(),
    })
}
