//! Ogg Opus writer (RFC 7845): the identification header on a page of its own, the comment
//! header on the next, then the Opus packets.
//!
//! The identification header is the stream's `extradata`, written as it came (a multistream
//! mapping table included): it is checked, never rebuilt. The comment header carries the tags
//! as Vorbis comments under our own vendor string.
//!
//! Granule positions count 48 kHz samples from 0, pre-skip included; a packet's duration is
//! read from its TOC, and a packet whose TOC is not valid counts for nothing, as in the reader.
//! A packet whose pts (counted from the first packet's) lies past the end of the packet before
//! it marks a hole, packets lost upstream: the open page closes and the position jumps, so the
//! next page's granule places its packets after the hole and a reader sees the same timeline.
//! A pts that goes back is not followed.
//!
//! Pages. A page closes before the packet that would take it past one second
//! ([`PAGE_FRAMES`]): RFC 7845 section 5 recommends no more than that for a file, and it is
//! when libopus's `opusenc` flushes, so a seek reads at most a second it did not need. The 255
//! lacing values close a page sooner for very small or very large packets; only a packet over
//! 65,025 bytes is split.
//!
//! The end (section 4.4). When the stream's length is known (`StreamInfo::frames`, as stream
//! copy has it from the source's last granule), the last page's granule is pre-skip plus that
//! length, so the file ends where the stream does, inside its last packet as a rule. With no
//! length the stream ends with its last packet. A granule may not go back, so once the open
//! page reaches the end, the packets after it (wholly trimmed) stay on that page.

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

/// One second at 48 kHz.
const PAGE_FRAMES: u64 = 48_000;

struct OggOpusMuxer {
    sink: Box<dyn MediaSink>,
    pages: PageWriter,
    /// The granule position after the packets written so far.
    pos: u64,
    /// The first packet's pts: packets are placed by their pts from it.
    origin: Option<i64>,
    /// The granule position at which the open page starts.
    page_start: u64,
    /// The granule position at which the stream ends, when its length is known.
    end: Option<u64>,
    /// The identification header as written.
    head: Vec<u8>,
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
        origin: None,
        page_start: 0,
        end: stream
            .frames
            .map(|n| n.saturating_add(u64::from(head.pre_skip))),
        head: stream.extradata.clone(),
    }))
}

impl Muxer for OggOpusMuxer {
    fn write_packet(&mut self, packet: &Packet) -> Result<(), Error> {
        let frames = packet_duration(&packet.data).map_or(0, |n| n as u64);
        let before = self.pages.sequence();
        let origin = *self.origin.get_or_insert(packet.pts);
        let at = u64::try_from(packet.pts.saturating_sub(origin)).unwrap_or(0);
        if at > self.pos {
            self.pages.end_page();
            self.pos = at;
        }
        let ended = self.end.is_some_and(|end| self.pos >= end);
        if self.pos - self.page_start + frames > PAGE_FRAMES && !ended {
            self.pages.end_page();
        }
        let start = self.pos;
        self.pos = self.pos.saturating_add(frames);
        self.pages.packet(&packet.data, self.pos);
        // Closed by us or by the lacing limit: the packet opens the next page.
        if self.pages.sequence() != before {
            self.page_start = start;
        }
        let ready = self.pages.take();
        if !ready.is_empty() {
            self.sink.write_all(&ready)?;
        }
        Ok(())
    }

    /// The header is on the first page already; an encoder may only confirm it.
    fn set_extradata(&mut self, extradata: &[u8]) -> Result<(), Error> {
        if extradata == self.head {
            return Ok(());
        }
        Err(Error::Unsupported {
            what: "ogg opus: the identification header is already written and cannot change".into(),
        })
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
