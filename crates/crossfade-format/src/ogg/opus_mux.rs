//! Ogg Opus writer (RFC 7845), the inverse of the reader in `opus.rs`: `OpusHead` alone on the
//! first page, `OpusTags` after it, then the audio packets on pages of their own.
//!
//! What the writer is given. `StreamInfo::extradata` is the `OpusHead`, exactly as the reader
//! hands it out; it is final when the first page is written, so an encoder states its
//! pre-skip up front. A later `set_extradata` may change only what the pages do not depend on
//! (input rate, output gain, version) and then rewrites the first page when the sink can seek;
//! a different pre-skip, channel count or mapping is refused.
//!
//! Packets carry the engine timeline in 48 kHz frames: the first packet's pts is `-pre_skip`,
//! each next one starts where the one before it ended, and its duration is its TOC's. Only the
//! last packet may state a shorter duration, and `StreamInfo::frames`, when known, ends the
//! audible audio too (the reader reports the last packet whole and the trimming only in
//! `frames`). A page's granule is `pre_skip + pts + duration` of the last packet that ends on
//! it; the last page's is cut to the audible end, which is RFC 7845's end trimming (never below
//! `pre_skip`: a page granule short of that is not an end but a lie). The last packet is held
//! back until `finish` or the next packet so that the cut can be applied to it.
//!
//! Pages. At most one second of audio each (RFC 7845 asks for the latency's sake), plus the
//! page writer's own limits. No stream start is written: a reader's start is always 0.
//!
//! A stream with no packet is a valid file: the comment page is also the last.

use super::mux::SERIAL;
use super::writer::{BOS, PageWriter, page_bytes};
use crate::vorbis_comment;
use crossfade_core::{CodecId, Error, MediaSink, Muxer, Packet, StreamInfo, Tags};
use crossfade_opus::{OpusHead, packet_duration};
use std::io::SeekFrom;

/// Samples of audio one page holds at most.
const PAGE_SAMPLES: u64 = 48_000;

fn refuse(what: impl std::fmt::Display) -> Error {
    Error::Malformed {
        what: format!("ogg opus: {what}"),
    }
}

/// A packet waiting for the next one (or `finish`) to say whether it is the last.
struct Held {
    data: Vec<u8>,
    pts: i64,
    /// The duration the packet states, at most its TOC's.
    duration: u64,
    toc: u64,
}

struct OpusMuxer {
    sink: Box<dyn MediaSink>,
    pages: PageWriter,
    head: OpusHead,
    head_bytes: Vec<u8>,
    frames: Option<u64>,
    /// Where the next packet must start.
    next_pts: i64,
    page_samples: u64,
    audio_started: bool,
    held: Option<Held>,
}

pub(super) fn create(
    mut sink: Box<dyn MediaSink>,
    stream: &StreamInfo,
    tags: &Tags,
) -> Result<Box<dyn Muxer>, Error> {
    debug_assert_eq!(stream.codec, CodecId::Opus);
    let head = OpusHead::parse(&stream.extradata).map_err(refuse)?;
    if stream.audio.sample_rate() != 48_000 {
        return Err(Error::Unsupported {
            what: format!(
                "ogg opus is 48 kHz, not {} Hz: resample first",
                stream.audio.sample_rate()
            ),
        });
    }
    if stream.audio.layout().channels() != u16::from(head.channels) {
        return Err(refuse(format!(
            "the stream has {} channels but its header {}",
            stream.audio.layout().channels(),
            head.channels
        )));
    }
    let mut pages = PageWriter::new(SERIAL);
    pages.packet(&stream.extradata, 0);
    pages.end_page();
    sink.write_all(&pages.take())?;
    // The comment page stays open until the audio starts or the stream ends, so that an empty
    // stream can close the file with it.
    let mut comment = b"OpusTags".to_vec();
    vorbis_comment::write(
        concat!("crossfade ", env!("CARGO_PKG_VERSION")),
        tags,
        &mut comment,
    );
    pages.packet(&comment, 0);
    sink.write_all(&pages.take())?;
    Ok(Box::new(OpusMuxer {
        sink,
        pages,
        next_pts: -i64::from(head.pre_skip),
        head,
        head_bytes: stream.extradata.clone(),
        frames: stream.frames,
        page_samples: 0,
        audio_started: false,
        held: None,
    }))
}

impl OpusMuxer {
    /// Puts a packet on a page, closing the page first when it would pass a second.
    fn put(&mut self, h: Held, end: i64) -> Result<(), Error> {
        if !self.audio_started {
            self.pages.end_page();
            self.audio_started = true;
        } else if self.page_samples > 0 && self.page_samples + h.duration > PAGE_SAMPLES {
            self.pages.end_page();
            self.page_samples = 0;
        }
        let granule = i64::from(self.head.pre_skip).saturating_add(end);
        self.pages
            .packet(&h.data, u64::try_from(granule).unwrap_or(0));
        self.page_samples += h.duration;
        self.sink.write_all(&self.pages.take())?;
        Ok(())
    }
}

impl Muxer for OpusMuxer {
    fn write_packet(&mut self, packet: &Packet) -> Result<(), Error> {
        let toc = packet_duration(&packet.data).map_err(refuse)? as u64;
        if packet.duration == 0 || packet.duration > toc {
            return Err(refuse(format!(
                "a packet of {toc} samples states a duration of {}",
                packet.duration
            )));
        }
        if packet.pts != self.next_pts {
            return Err(refuse(format!(
                "a packet starts at {} where {} was expected (the first at -pre_skip, each one \
                 where the one before ended)",
                packet.pts, self.next_pts
            )));
        }
        if let Some(h) = self.held.take() {
            if h.duration != h.toc {
                return Err(refuse(
                    "only the last packet may be shorter than its TOC says",
                ));
            }
            let end = h.pts + h.duration as i64;
            self.put(h, end)?;
        }
        self.next_pts = packet.pts.saturating_add(toc as i64);
        self.held = Some(Held {
            data: packet.data.clone(),
            pts: packet.pts,
            duration: packet.duration,
            toc,
        });
        Ok(())
    }

    fn set_extradata(&mut self, extradata: &[u8]) -> Result<(), Error> {
        if extradata == self.head_bytes {
            return Ok(());
        }
        let head = OpusHead::parse(extradata).map_err(refuse)?;
        if head.pre_skip != self.head.pre_skip
            || head.channels != self.head.channels
            || head.mapping != self.head.mapping
        {
            return Err(refuse(
                "the final header changes the pre-skip, the channels or the mapping, which \
                 the pages already written depend on",
            ));
        }
        // A sink that cannot seek keeps the provisional header, which is still valid.
        if self.sink.seek(SeekFrom::Start(0)).is_ok() {
            let mut lacing = vec![255; extradata.len() / 255];
            lacing.push((extradata.len() % 255) as u8);
            self.sink
                .write_all(&page_bytes(BOS, 0, SERIAL, 0, &lacing, extradata))?;
            self.sink.seek(SeekFrom::End(0))?;
        }
        self.head = head;
        self.head_bytes = extradata.to_vec();
        Ok(())
    }

    fn finish(&mut self) -> Result<(), Error> {
        if let Some(h) = self.held.take() {
            let mut end = h.pts + h.duration as i64;
            if let Some(frames) = self.frames {
                end = end.min(i64::try_from(frames).unwrap_or(i64::MAX));
            }
            if end < h.pts {
                return Err(refuse("the stream's length ends before its last packet"));
            }
            self.put(h, end.max(0))?;
        }
        self.pages.finish();
        self.sink.write_all(&self.pages.take())?;
        self.sink.finish()?;
        Ok(())
    }
}
