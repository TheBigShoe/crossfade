//! The Ogg Vorbis mapping (Vorbis I specification, appendix A, and section 4.3.9 for the
//! channel order).
//!
//! Timing. A page's granule position is the sample position after the last packet that ends
//! on it; a packet yields the previous block's quarter plus its own (none for the first, and
//! none for one the decoder cannot use, which leaves the overlap alone, as libvorbis skips it).
//! The packets of a page are read as a batch and placed backwards from its granule, so a page
//! after skipped packets keeps the position its granule gives (the output then has a gap where
//! the encoder counted samples the decoder never made: the decoded stream is shorter by them,
//! as libvorbis's is; it is not filled with silence, which nothing in the stream asks for).
//! A page whose granule falls short of what its packets yield after the previous page is the
//! end padding of the last page, and its packets run on forward instead (so is a short granule
//! on a page that is both the first and the last, as libvorbisfile reads it). The first page fixes
//! the timeline: what its packets yield beyond its granule is priming (negative pts); a stream
//! whose first sample sits past 0 (cut out of a longer one) is shifted to start at 0.
//!
//! The end. On a seekable source the last page's granule, read at open, gives
//! `StreamInfo::frames`, and the engine cuts the padding there. A pipe cannot read ahead:
//! `frames` is `None` and the padding is decoded and kept.
//!
//! Truncation. A file cut off inside a packet (Xiph's `one-entry-codebook-test.ogg` and
//! `test-short2.ogg` are) ends with the last whole packet, as libvorbisfile plays it.
//!
//! Chains. A chained file (a new beginning-of-stream page after the first link ends) is read to
//! the end of its first link: the stream description is fixed at open, and a later link may
//! have another rate or channel count.

use super::LogicalStream;
use super::packets::{PacketReader, PacketStart};
use crate::vorbis_comment;
use crossfade_core::{
    AudioSpec, ChannelLayout, CodecId, Demuxer, Error, Limits, Packet, SampleFormat, StreamInfo,
    Tags, TimeBase,
};
use crossfade_vorbis::{HeaderKind, Ident, Modes, Setup, header_kind};
use std::collections::VecDeque;

fn malformed(what: impl std::fmt::Display) -> Error {
    Error::Malformed {
        what: format!("ogg vorbis: {what}"),
    }
}

fn vorbis_error(e: crossfade_vorbis::Error) -> Error {
    match e {
        crossfade_vorbis::Error::OverBudget { requested, budget } => Error::AllocLimitExceeded {
            requested,
            max: budget,
        },
        other => Error::Malformed {
            what: format!("ogg {other}"),
        },
    }
}

/// Packets read ahead in one batch at most, so a stream that never gives a granule cannot make
/// the reader hold it all.
const MAX_BATCH: usize = 256;

struct Queued {
    data: Vec<u8>,
    /// Granule position of the frame the packet's first output frame (or the next one) holds.
    pos: i64,
    frames: u64,
}

struct OggVorbisDemuxer {
    packets: PacketReader,
    streams: [StreamInfo; 1],
    tags: Tags,
    modes: Modes,
    /// The granule position of pts 0.
    shift: i64,
    /// Where the first audio packet starts; `None` when there is none.
    floor: Option<PacketStart>,
    queue: VecDeque<Queued>,
    spare: Vec<Vec<u8>>,
    /// The block size of the last packet queued that the decoder can use.
    prev_block: Option<usize>,
    /// The granule position after the last packet queued; `None` before the first batch (at
    /// the start or after a seek), when the next page's granule fixes it.
    next_pos: Option<i64>,
    /// A packet already read (the first audio packet, read at open on a pipe).
    held: Option<Vec<u8>>,
    done: bool,
}

pub(crate) fn open(stream: LogicalStream, limits: &Limits) -> Result<Box<dyn Demuxer>, Error> {
    let LogicalStream {
        mut packets, id, ..
    } = stream;
    let ident = Ident::parse(&id).map_err(vorbis_error)?;
    let mut comment = Vec::new();
    if !packets.next_packet(&mut comment)? {
        return Err(malformed("the stream ends before its comment header"));
    }
    if header_kind(&comment) != Some(HeaderKind::Comment) {
        return Err(malformed("the second packet is not the comment header"));
    }
    let tags = vorbis_comment::parse(&comment[7..], limits)
        .map(|c| c.tags)
        .unwrap_or_default();
    let mut setup_packet = Vec::new();
    if !packets.next_packet(&mut setup_packet)? {
        return Err(malformed("the stream ends before its setup header"));
    }
    if header_kind(&setup_packet) != Some(HeaderKind::Setup) {
        return Err(malformed("the third packet is not the setup header"));
    }
    let setup =
        Setup::parse(&setup_packet, &ident, limits.max_alloc_bytes).map_err(vorbis_error)?;
    let modes = setup.modes().clone();
    let audio = AudioSpec::new(
        ident.sample_rate,
        SampleFormat::F32,
        ChannelLayout::vorbis_order(ident.channels)?,
    )?;
    let mut info = StreamInfo::new(
        0,
        CodecId::Vorbis,
        audio,
        TimeBase::new(1, i64::from(ident.sample_rate))?,
    );
    info.extradata = crossfade_vorbis::pack_headers([&id, &comment, &setup_packet]);

    let mut first = Vec::new();
    let has_audio = packets.next_packet(&mut first)?;
    let floor = has_audio.then(|| packets.packet_start());
    let mut held = has_audio.then_some(first);
    // On a seekable source, the last granule (then back to the first audio packet).
    let mut last = None;
    if let Some(floor) = floor
        && packets.check_seekable().is_ok()
    {
        last = packets.seek_granule(i64::MAX, floor)?;
        packets.seek_granule(i64::MIN, floor)?;
        held = None;
    }
    let mut d = OggVorbisDemuxer {
        packets,
        streams: [info],
        tags,
        modes,
        shift: 0,
        floor,
        queue: VecDeque::new(),
        spare: Vec::new(),
        prev_block: None,
        next_pos: None,
        held,
        done: !has_audio,
    };
    if has_audio {
        d.fill()?;
        let start = d.queue.front().map_or(0, |q| q.pos);
        d.shift = start.max(0);
        if let Some(g) = last {
            d.streams[0].frames = Some(u64::try_from(g.saturating_sub(d.shift)).unwrap_or(0));
        }
    } else if floor.is_none() {
        d.streams[0].frames = Some(0);
    }
    Ok(Box::new(d))
}

impl OggVorbisDemuxer {
    /// Frames `packet` yields after the last usable one, as the decoder counts them.
    fn frames_of(&mut self, packet: &[u8]) -> i64 {
        match self.modes.block_size(packet) {
            Ok(n) => {
                let y = self
                    .prev_block
                    .map_or(0, |p| crossfade_vorbis::packet_frames(p, n));
                self.prev_block = Some(n);
                y as i64
            }
            Err(_) => 0,
        }
    }

    /// Reads the next page's packets (up to the next granule) and places them.
    fn fill(&mut self) -> Result<(), Error> {
        let first = self.queue.len();
        let mut granule = None;
        let mut total = 0i64;
        while self.queue.len() - first < MAX_BATCH {
            let mut data = self.spare.pop().unwrap_or_default();
            if let Some(h) = self.held.take() {
                data = h;
            } else {
                // A file cut off inside its last packet ends before it, as players read it.
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
            let frames = self.frames_of(&data);
            total = total.saturating_add(frames);
            self.queue.push_back(Queued {
                data,
                pos: 0,
                frames: frames as u64,
            });
            if let Some(g) = self.packets.granule() {
                granule = Some(g);
                break;
            }
        }
        // A page that is both the first of a batch run and the last of the stream: a granule
        // short of its packets is end padding (libvorbisfile's reading), not priming.
        let mut last_page = false;
        if granule.is_some() && self.next_pos.is_none() && self.held.is_none() {
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
        let back = granule.map(|g| {
            let b = g.saturating_sub(total);
            if last_page { b.max(0) } else { b }
        });
        let mut pos = match (back, self.next_pos) {
            (Some(b), None) => b,
            // A granule short of what the packets yield after the last page: the end padding.
            (Some(b), Some(run)) => b.max(run),
            (None, Some(run)) => run,
            (None, None) => self.shift,
        };
        for q in self.queue.iter_mut().skip(first) {
            q.pos = pos;
            pos = pos.saturating_add(q.frames as i64);
        }
        if self.queue.len() > first {
            self.next_pos = Some(pos);
        }
        Ok(())
    }

    /// Forgets what was read ahead, for a seek.
    fn clear(&mut self) {
        self.spare.extend(self.queue.drain(..).map(|q| q.data));
        self.prev_block = None;
        self.next_pos = None;
        self.held = None;
        self.done = false;
    }

    fn front_pts(&self) -> Option<i64> {
        self.queue.front().map(|q| q.pos.saturating_sub(self.shift))
    }
}

impl Demuxer for OggVorbisDemuxer {
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
        out.pts = q.pos.saturating_sub(self.shift);
        out.duration = q.frames;
        let old = std::mem::replace(&mut out.data, q.data);
        self.spare.push(old);
        Ok(true)
    }

    /// Lands at least a long block's half, and a page, before the target: the first packet read
    /// after a seek only primes the decoder's overlap, so the target must lie past what it
    /// would have yielded.
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
        let preroll = i64::from(self.modes.block_sizes()[1] / 2);
        let target = pts.max(0).saturating_add(self.shift);
        // One page further back than the landing: the landing page's packets are then placed
        // from its granule like any page's, and the batch that ends the stream (whose granule
        // is short by the padding) never comes first after a seek.
        if let Some(g) = self
            .packets
            .seek_granule(target.saturating_sub(preroll), floor)?
        {
            self.packets.seek_granule(g.saturating_sub(1), floor)?;
        }
        self.clear();
        self.fill()?;
        match self.front_pts() {
            Some(p) if p <= pts.max(0) => Ok(p),
            // Granules that disagree with the packets: read from the start instead.
            Some(_) => {
                self.packets.seek_granule(i64::MIN, floor)?;
                self.clear();
                self.fill()?;
                Ok(self.front_pts().unwrap_or(0))
            }
            None => {
                self.done = true;
                Ok(self.next_pos.map_or(0, |p| p.saturating_sub(self.shift)))
            }
        }
    }
}
