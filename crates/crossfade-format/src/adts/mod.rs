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

/// Garbage the reader skips looking for the next frame before it gives up.
pub(crate) const MAX_RESYNC: usize = 1;
/// Seek index entries at most.
pub(crate) const MAX_INDEX: usize = 1;

pub(crate) struct AdtsReader;

impl AdtsReader {
    pub(crate) fn open(source: Box<dyn MediaSource>, limits: &Limits) -> Result<Self, Error> {
        let _ = (source, limits);
        Err(Error::UnrecognisedFormat)
    }
    pub(crate) fn index_len(&self) -> usize {
        0
    }
}

impl Demuxer for AdtsReader {
    fn streams(&self) -> &[crossfade_core::StreamInfo] {
        &[]
    }
    fn read_packet(&mut self, _: &mut crossfade_core::Packet) -> Result<bool, Error> {
        Ok(false)
    }
}

#[cfg(test)]
mod tests;
