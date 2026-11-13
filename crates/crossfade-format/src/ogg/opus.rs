//! The Ogg Opus mapping (RFC 7845).
//!
//! Headers. The identification header (`OpusHead`, through `crossfade_opus::OpusHead`) is the
//! stream's `extradata`; the comment header (`OpusTags`) must follow, and its comments are the
//! tags (a comment header whose lengths do not hold gives no tags; `R128_*` gains are carried
//! as tags, not applied). Output is always 48 kHz: the header's input rate is information
//! about the encoder's source and is not shown.
//!
//! Timing (section 4). A granule position counts 48 kHz samples, pre-skip included, up to the
//! end of the last packet completed on its page; a packet's duration comes from its TOC. The
//! first page's granule minus its packets' samples is the stream's start (above 0 for a stream
//! cut at the front, section 4.5); the first packet's pts is `-pre_skip` whatever the start,
//! since exactly pre-skip samples are to be dropped. A first page whose granule is short of its
//! packets is refused unless it is also the last page (the RFC's MUST): then the stream starts
//! at 0 and the granule ends it, and one short of the pre-skip is refused. Later pages are
//! placed forward from the last; a granule past that leaves a gap, as for Vorbis.
//!
//! The end (section 4.4). On a seekable source the last page's granule gives
//! `StreamInfo::frames` (minus the start and the pre-skip), and the engine trims there; on a
//! pipe `frames` is `None` (unless the first page is the last) and the end is decoded whole.
//!
//! Seeking (section 4.6) lands on a page at least 80 ms (3,840 samples) before the target, so
//! the decoder has converged by then; before the start it lands on the first packet.
//!
//! Chains. A chained file is read to the end of its first link, as for Vorbis.

use super::LogicalStream;
use super::packets::{PacketReader, PacketStart};
use crate::vorbis_comment;
use crossfade_core::{
    AudioSpec, ChannelLayout, CodecId, Demuxer, Error, Limits, Packet, SampleFormat, StreamInfo,
    Tags, TimeBase,
};
use crossfade_opus::{OpusHead, OpusTags, packet_duration};
use std::collections::VecDeque;

fn malformed(what: impl std::fmt::Display) -> Error {
    Error::Malformed {
        what: format!("ogg opus: {what}"),
    }
}

/// Packets read ahead in one batch at most, so a stream that never gives a granule cannot make
/// the reader hold it all.
const MAX_BATCH: usize = 256;

/// RFC 7845 4.6: decoding starts at least this long (80 ms) before a seek target.
pub(crate) const PRE_ROLL: i64 = 3840;

struct Queued {
    data: Vec<u8>,
    /// Granule position before the packet.
    pos: i64,
    frames: u64,
}

struct OggOpusDemuxer {
    packets: PacketReader,
    streams: [StreamInfo; 1],
    tags: Tags,
    pre_skip: i64,
    /// The granule position before the first packet (the stream's start).
    start: i64,
    /// Where the first audio packet starts; `None` when there is none.
    floor: Option<PacketStart>,
    queue: VecDeque<Queued>,
    spare: Vec<Vec<u8>>,
    /// The granule position after the last packet queued; `None` before the first batch.
    next_pos: Option<i64>,
    /// A packet already read ahead.
    held: Option<Vec<u8>>,
    done: bool,
}

/// The layout of `head`'s output channels: mono or stereo for family 0, Vorbis order for
/// family 1, unpositioned otherwise.
fn layout(head: &OpusHead) -> Result<ChannelLayout, Error> {
    Ok(match (head.mapping.family, head.channels) {
        (0, 1) => ChannelLayout::mono(),
        (0, _) => ChannelLayout::stereo(),
        (1, n) => ChannelLayout::vorbis_order(n)?,
        (_, n) => ChannelLayout::unpositioned(u16::from(n))?,
    })
}

pub(crate) fn open(stream: LogicalStream, limits: &Limits) -> Result<Box<dyn Demuxer>, Error> {
    let LogicalStream {
        mut packets, id, ..
    } = stream;
    let head = OpusHead::parse(&id).map_err(|e| malformed(strip(e)))?;
    let mut comment = Vec::new();
    if !packets.next_packet(&mut comment)? {
        return Err(malformed("the stream ends before its comment header"));
    }
    if !comment.starts_with(b"OpusTags") {
        return Err(malformed("the second packet is not the comment header"));
    }
    let tags = OpusTags::parse(&comment)
        .ok()
        .and_then(|t| vorbis_comment::parse(t.body, limits).ok())
        .map(|c| c.tags)
        .unwrap_or_default();
    let audio = AudioSpec::new(48_000, SampleFormat::F32, layout(&head)?)?;
    let mut info = StreamInfo::new(0, CodecId::Opus, audio, TimeBase::new(1, 48_000)?);
    info.extradata = id;

    let mut first = Vec::new();
    let has_audio = packets.next_packet(&mut first)?;
    let floor = has_audio.then(|| packets.packet_start());
    let mut held = has_audio.then_some(first);
    let mut last = None;
    if let Some(floor) = floor
        && packets.check_seekable().is_ok()
    {
        last = packets.seek_granule(i64::MAX, floor)?;
        packets.seek_granule(i64::MIN, floor)?;
        held = None;
    }
    let mut d = OggOpusDemuxer {
        packets,
        streams: [info],
        tags,
        pre_skip: i64::from(head.pre_skip),
        start: 0,
        floor,
        queue: VecDeque::new(),
        spare: Vec::new(),
        next_pos: None,
        held,
        done: !has_audio,
    };
    if has_audio {
        let end = d.fill_first()?;
        let end = end.or(last);
        if let Some(g) = end {
            let audible = g.saturating_sub(d.start).saturating_sub(d.pre_skip);
            d.streams[0].frames = Some(u64::try_from(audible).unwrap_or(0));
        }
    } else {
        d.streams[0].frames = Some(0);
    }
    Ok(Box::new(d))
}

/// `crossfade_opus`'s text without its own "opus: " prefix, which `malformed` adds.
fn strip(e: crossfade_opus::Error) -> String {
    let s = e.to_string();
    s.strip_prefix("opus: ").map(str::to_owned).unwrap_or(s)
}

impl OggOpusDemuxer {
    /// Reads packets up to the next granule (or the end, or a batch's worth): the packets and
    /// the page's granule, `None` when none was met.
    fn read_batch(&mut self) -> Result<Option<i64>, Error> {
        let first = self.queue.len();
        while self.queue.len() - first < MAX_BATCH {
            let mut data = self.spare.pop().unwrap_or_default();
            if let Some(h) = self.held.take() {
                data = h;
            } else {
                // A file cut off inside its last packet ends before it.
                let more = match self.packets.next_packet(&mut data) {
                    Err(Error::UnexpectedEof { .. }) => false,
                    other => other?,
                };
                if !more {
                    self.spare.push(data);
                    self.done = true;
                    break;
                }
            }
            let frames = packet_duration(&data).map_or(0, |n| n as u64);
            self.queue.push_back(Queued {
                data,
                pos: 0,
                frames,
            });
            if let Some(g) = self.packets.granule() {
                return Ok(Some(g));
            }
        }
        Ok(None)
    }

    /// The first batch: it fixes the stream's start (section 4.5). Returns the granule that
    /// ends the stream when the first page is also the last.
    fn fill_first(&mut self) -> Result<Option<i64>, Error> {
        let granule = self.read_batch()?;
        let total: i64 = self.queue.iter().map(|q| q.frames as i64).sum();
        // Whether this page is the stream's last: nothing follows it.
        let mut last_page = self.done;
        if granule.is_some() && !self.done {
            let mut next = self.spare.pop().unwrap_or_default();
            let more = match self.packets.next_packet(&mut next) {
                Err(Error::UnexpectedEof { .. }) => false,
                other => other?,
            };
            if more {
                self.held = Some(next);
            } else {
                self.spare.push(next);
                last_page = true;
            }
        }
        let mut end = None;
        self.start = match granule {
            Some(g) if g >= total => g - total,
            Some(g) if last_page => {
                if g < self.pre_skip {
                    return Err(malformed(
                        "the only page's granule position is less than the pre-skip",
                    ));
                }
                end = Some(g);
                0
            }
            Some(_) => {
                return Err(malformed(
                    "the first page's granule position is less than its packets' samples",
                ));
            }
            None => 0,
        };
        self.place(self.start);
        Ok(end)
    }

    /// Reads the next batch, placed forward from the last (or backward from its granule after
    /// a gap).
    fn fill(&mut self) -> Result<(), Error> {
        let first = self.queue.len();
        let granule = self.read_batch()?;
        let total: i64 = self.queue.iter().skip(first).map(|q| q.frames as i64).sum();
        let run = self.next_pos.unwrap_or(self.start);
        // A granule past what the packets yield after the last page is a gap; one short of it
        // is the end trimming of the last page, whose packets run on forward.
        let pos = granule.map_or(run, |g| g.saturating_sub(total).max(run));
        self.place_from(first, pos);
        Ok(())
    }

    fn place(&mut self, pos: i64) {
        self.place_from(0, pos);
    }

    fn place_from(&mut self, first: usize, mut pos: i64) {
        for q in self.queue.iter_mut().skip(first) {
            q.pos = pos;
            pos = pos.saturating_add(q.frames as i64);
        }
        if self.queue.len() > first {
            self.next_pos = Some(pos);
        }
    }

    /// Forgets what was read ahead, for a seek.
    fn clear(&mut self) {
        self.spare.extend(self.queue.drain(..).map(|q| q.data));
        self.held = None;
        self.done = false;
    }

    fn pts(&self, pos: i64) -> i64 {
        pos.saturating_sub(self.start).saturating_sub(self.pre_skip)
    }
}

impl Demuxer for OggOpusDemuxer {
    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }

    fn tags(&self) -> &Tags {
        &self.tags
    }

    fn read_packet(&mut self, out: &mut Packet) -> Result<bool, Error> {
        if self.queue.is_empty() && !self.done {
            self.fill()?;
        }
        let Some(q) = self.queue.pop_front() else {
            return Ok(false);
        };
        out.stream = 0;
        out.pts = self.pts(q.pos);
        out.duration = q.frames;
        let old = std::mem::replace(&mut out.data, q.data);
        self.spare.push(old);
        Ok(true)
    }

    /// Lands on the packet after the last page at least [`PRE_ROLL`] before the target.
    fn seek(&mut self, pts: i64) -> Result<i64, Error> {
        self.packets.check_seekable()?;
        let Some(floor) = self.floor else {
            self.done = true;
            return Ok(0);
        };
        if let Some(total) = self.streams[0].frames
            && u64::try_from(pts).is_ok_and(|p| p >= total)
        {
            self.clear();
            self.done = true;
            return i64::try_from(total).map_err(|_| Error::Overflow);
        }
        let target = pts
            .max(0)
            .saturating_add(self.pre_skip)
            .saturating_add(self.start);
        let landing = self
            .packets
            .seek_granule(target.saturating_sub(PRE_ROLL), floor)?;
        self.clear();
        self.next_pos = Some(landing.unwrap_or(self.start));
        self.fill()?;
        match self.queue.front().map(|q| self.pts(q.pos)) {
            Some(p) if p <= pts.max(0) => Ok(p),
            // Granules that disagree with the packets: read from the start instead.
            Some(_) => {
                self.packets.seek_granule(i64::MIN, floor)?;
                self.clear();
                self.next_pos = Some(self.start);
                self.fill()?;
                Ok(self.queue.front().map_or(0, |q| self.pts(q.pos)))
            }
            None => {
                self.done = true;
                Ok(self.next_pos.map_or(0, |p| self.pts(p)))
            }
        }
    }
}
