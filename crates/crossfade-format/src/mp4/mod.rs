//! ISO base media files (`.mp4`, `.m4a`, `.m4b`), audio only.
//!
//! The first sound track is read: ALAC, AAC (in `mp4a` with an `esds`) and MP3 in MP4. One MP4
//! sample is one packet, found through the sample tables, which are kept so that any sample's
//! place and time costs O(log n). `moov` is read whole (within the allocation limit); before
//! `mdat` it works on a pipe, after it only on an input that can seek. Fragmented and encrypted
//! files are refused by name.
//!
//! Positions: media time converts to sample frames as `t * rate / timescale`, rounded down,
//! so durations are differences of positions and never drift. See `demux::gapless` for where
//! the priming and the audible length come from.

mod asc;
mod boxes;
mod demux;
mod entry;
mod ilst;
mod moov;
mod table;

use crossfade_core::{Demuxer, Error, Limits, MediaSource};

/// Opens an MP4 file; `source` starts at its first byte.
pub fn open(source: Box<dyn MediaSource>, limits: &Limits) -> Result<Box<dyn Demuxer>, Error> {
    demux::open(source, limits)
}

#[cfg(test)]
mod tests;
