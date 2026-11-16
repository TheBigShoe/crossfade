//! Ogg Opus writer (RFC 7845): the identification header on a page of its own, the comment
//! header on the next, then the Opus packets.
//!
//! The identification header is the stream's `extradata`, written as it came (a multistream
//! mapping table included): it is checked, never rebuilt. The comment header carries the tags
//! as Vorbis comments under our own vendor string.
//!
//! Granule positions count 48 kHz samples from 0, pre-skip included; a packet's duration is
//! read from its TOC, and a packet whose TOC is not valid counts for nothing, as in the reader.
//!
//! The end (section 4.4). When the stream's length is known (`StreamInfo::frames`, as stream
//! copy has it from the source's last granule), the last page's granule is pre-skip plus that
//! length, so the file ends where the stream does, inside its last packet as a rule. With no
//! length the stream ends with its last packet.

use super::mux::SERIAL;
use super::writer::PageWriter;
use crate::vorbis_comment;
use crossfade_core::{Error, MediaSink, Muxer, Packet, StreamInfo, Tags};
use crossfade_opus::{OpusHead, packet_duration};

fn malformed(e: crossfade_opus::Error) -> Error {
    // `crossfade_opus`'s text carries its own "opus: " prefix.
    let s = e.to_string();
    Error::Malformed {
        what: format!("ogg opus: {}", s.strip_prefix("opus: ").unwrap_or(&s)),
    }
}

struct OggOpusMuxer {
    sink: Box<dyn MediaSink>,
    pages: PageWriter,
    /// The granule position after the packets written so far.
    pos: u64,
    /// The granule position at which the stream ends, when its length is known.
    end: Option<u64>,
}

pub(super) fn create(
    mut sink: Box<dyn MediaSink>,
    stream: &StreamInfo,
    tags: &Tags,
) -> Result<Box<dyn Muxer>, Error> {
    let head = OpusHead::parse(&stream.extradata).map_err(malformed)?;
    let rate = stream.audio.sample_rate();
    if rate != 48_000 {
        return Err(Error::Unsupported {
            what: format!("ogg opus is timed in 48 kHz samples, not at {rate} Hz"),
        });
    }
    let mut comment = b"OpusTags".to_vec();
    vorbis_comment::write(
        &format!("Crossfade {}", env!("CARGO_PKG_VERSION")),
        tags,
        &mut comment,
    );
    let mut pages = PageWriter::paced_by_caller(SERIAL);
    pages.packet(&stream.extradata, 0);
    pages.end_page();
    pages.packet(&comment, 0);
    pages.end_page();
    sink.write_all(&pages.take())?;
    Ok(Box::new(OggOpusMuxer {
        sink,
        pages,
        pos: 0,
        end: stream
            .frames
            .map(|n| n.saturating_add(u64::from(head.pre_skip))),
    }))
}

impl Muxer for OggOpusMuxer {
    fn write_packet(&mut self, packet: &Packet) -> Result<(), Error> {
        let frames = packet_duration(&packet.data).map_or(0, |n| n as u64);
        self.pos = self.pos.saturating_add(frames);
        self.pages.packet(&packet.data, self.pos);
        let ready = self.pages.take();
        if !ready.is_empty() {
            self.sink.write_all(&ready)?;
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), Error> {
        if let Some(end) = self.end {
            self.pages.set_granule(end.min(self.pos));
        }
        self.pages.finish();
        self.sink.write_all(&self.pages.take())?;
        self.sink.finish()?;
        Ok(())
    }
}
