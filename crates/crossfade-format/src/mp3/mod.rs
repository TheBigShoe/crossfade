//! MPEG audio (layers I-III) in its own elementary stream, the `.mp3` file. Not built yet.

use crossfade_core::{Demuxer, Error, Limits, MediaSource};

/// Opens an MPEG audio stream. `source` is positioned at the first byte after any ID3v2 tags,
/// which `open_demuxer` read while sniffing and passes as `id3`: every tag back to back, exactly
/// as read (header, body and any footer), `None` when the stream has none. Positions and seeks
/// on `source` are offsets in the whole input, so the audio starts at byte `id3.len()`.
pub fn open(
    source: Box<dyn MediaSource>,
    id3: Option<Vec<u8>>,
    limits: &Limits,
) -> Result<Box<dyn Demuxer>, Error> {
    let _ = (source, id3, limits);
    Err(Error::Unsupported {
        what: "reading mp3 is not built yet".into(),
    })
}
