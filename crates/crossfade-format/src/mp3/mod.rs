//! MPEG audio in its own elementary stream, the `.mp3` file: Layer III frames, optionally
//! behind ID3v2 tags and an info frame, optionally followed by APEv2 and ID3v1 tags.
//!
//! Timeline: the decoder delays its output by [`DECODER_DELAY`] sample frames, and an encoder
//! adds its own delay in front and padding at the end. With LAME's figures the first packet's
//! pts is `-(delay + 529)` and the stream is `frames * samples_per_frame - delay - padding`
//! long; without them the first pts is `-529` and the length is every decoded sample but the
//! decoder's delay, when the number of frames is known (from an info frame, or by scanning
//! the headers of a seekable input once).

mod framer;
mod id3;

use crossfade_core::{
    AudioSpec, ChannelLayout, CodecId, Demuxer, Error, Limits, MediaSource, Packet, SampleFormat,
    StreamInfo, Tags, TimeBase,
};
use crossfade_mp3::info::InfoFrame;
use crossfade_mp3::{FrameHeader, Layer};
use framer::{Frame, Framer, Stop};
use std::io::SeekFrom;

/// The decoder's own delay in sample frames: 528 from the hybrid filterbank and 1 more from
/// the polyphase synthesis's alignment (the figure gapless players use).
pub const DECODER_DELAY: i64 = 529;

/// How far into the audio the first frame may start (junk or zero padding before it): the
/// bound sniffing uses too.
pub const MAX_LEADING_JUNK: u64 = 64 << 10;

/// Bytes of main data a Layer III frame can reach back into earlier frames: 511 in MPEG-1,
/// 255 in MPEG-2 and 2.5. Seeking starts early enough to have this much before any frame it
/// keeps.
fn max_reservoir(lsf: bool) -> usize {
    if lsf { 255 } else { 511 }
}

/// The frame to start decoding from so that frame `target` decodes exactly as in a straight
/// decode: the frame before the target's (two before in MPEG-2 and 2.5, `lsf`, whose frames are
/// one granule) must decode in full, since its overlap and filterbank history feed the target;
/// and before that frame come enough frames to hold a full reservoir of main data.
/// `main_data(i)` is the bytes of main data frame `i` holds. Shared with the MP4 reader.
pub(crate) fn first_frame_to_decode(
    target: usize,
    lsf: bool,
    main_data: impl Fn(usize) -> usize,
) -> usize {
    let reservoir = max_reservoir(lsf);
    let mut j = target.saturating_sub(if lsf { 2 } else { 1 });
    let mut held = 0usize;
    while j > 0 && held < reservoir {
        j -= 1;
        held += main_data(j);
    }
    j
}

/// Opens an MPEG audio stream. `source` is positioned at the first byte after any ID3v2 tags,
/// which `open_demuxer` read while sniffing and passes as `id3`: every tag back to back, exactly
/// as read (header, body and any footer), `None` when the stream has none. Positions and seeks
/// on `source` are offsets in the whole input, so the audio starts at byte `id3.len()`.
pub fn open(
    mut source: Box<dyn MediaSource>,
    id3: Option<Vec<u8>>,
    limits: &Limits,
) -> Result<Box<dyn Demuxer>, Error> {
    let start = id3.as_ref().map_or(0, |t| t.len() as u64);
    let mut tags = Tags::new();
    if let Some(t) = &id3 {
        id3::parse_v2(t, &mut tags);
    }
    let end = if source.is_seekable() {
        let (end, v1) = trailing_tags(&mut *source, start)?;
        if let Some(v1) = v1 {
            id3::parse_v1(&v1, &mut tags);
        }
        source.seek(SeekFrom::Start(start))?;
        end
    } else {
        None
    };
    let mut framer = Framer::new(source, start, MAX_LEADING_JUNK);
    framer.set_end(end);
    let first = match framer.next()? {
        Ok(f) => f,
        Err(_) => {
            return Err(Error::Malformed {
                what: "mp3: no MPEG audio frame found".into(),
            });
        }
    };
    let h = first.header;
    if h.layer != Layer::III {
        return Err(Error::Unsupported {
            what: format!(
                "mp3: {} layer {} audio is not supported, only layer III",
                h.version.name(),
                h.layer.name()
            ),
        });
    }
    // From here on garbage between frames is skipped however long it is.
    framer.set_max_junk(u64::MAX);
    let info = InfoFrame::parse(&h, framer.bytes(&first));
    let spf = i64::from(h.samples_per_frame());
    let (delay, padding) = info
        .and_then(|i| i.delay_padding)
        .map_or((0, 0), |(d, p)| (i64::from(d), i64::from(p)));
    let first_pts = -(delay + DECODER_DELAY);
    let mut demux = Mp3Demuxer {
        framer,
        streams: [stream_info(&h)?],
        tags,
        first_pts,
        spf,
        channels: h.channels(),
        pending: None,
        packets: 0,
        audio_start: first.offset,
        index: None,
        max_alloc: limits.max_alloc_bytes,
        max_packet: limits.max_packet_bytes,
        done: false,
    };
    if info.is_some() {
        demux.audio_start = first.offset + first.len as u64;
    } else {
        demux.pending = Some((first, demux.framer.bytes(&first).to_vec()));
    }
    let frame_count = match info.and_then(|i| i.frames) {
        Some(n) => Some(u64::from(n)),
        None if demux.framer.source().is_seekable() => Some(demux.build_index()?.len() as u64),
        None => None,
    };
    demux.streams[0].frames = frame_count.map(|n| {
        let total = n as i64 * spf;
        let cut = if delay + padding > 0 {
            delay + padding
        } else {
            DECODER_DELAY
        };
        (total - cut).max(0) as u64
    });
    Ok(Box::new(demux))
}

fn stream_info(h: &FrameHeader) -> Result<StreamInfo, Error> {
    let layout = if h.channels() == 1 {
        ChannelLayout::mono()
    } else {
        ChannelLayout::stereo()
    };
    let audio = AudioSpec::new(h.sample_rate, SampleFormat::F32, layout)?;
    Ok(StreamInfo::new(
        0,
        CodecId::Mp3,
        audio,
        TimeBase::new(1, i64::from(h.sample_rate))?,
    ))
}

/// Where the audio ends once an ID3v1 tag (the last 128 bytes, starting `TAG`) and an APEv2
/// tag (before the ID3v1 tag or at the very end) are left out, and the ID3v1 tag.
fn trailing_tags(
    source: &mut dyn MediaSource,
    start: u64,
) -> Result<(Option<u64>, Option<[u8; 128]>), Error> {
    let Some(mut end) = source.byte_len() else {
        return Ok((None, None));
    };
    let mut v1 = None;
    if end >= start + 128 {
        let mut tag = [0u8; 128];
        if read_at(source, end - 128, &mut tag)? && tag.starts_with(b"TAG") {
            v1 = Some(tag);
            end -= 128;
        }
    }
    if end >= start + 32 {
        let mut footer = [0u8; 32];
        if read_at(source, end - 32, &mut footer)?
            && let Some(len) = id3::ape_len(&footer)
            && len <= end - start
        {
            end -= len;
        }
    }
    Ok((Some(end), v1))
}

/// Reads exactly `buf.len()` bytes at `offset`; false if the input is shorter.
fn read_at(source: &mut dyn MediaSource, offset: u64, buf: &mut [u8]) -> Result<bool, Error> {
    source.seek(SeekFrom::Start(offset))?;
    let mut n = 0;
    while n < buf.len() {
        match source.read(&mut buf[n..]) {
            Ok(0) => return Ok(false),
            Ok(k) => n += k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(true)
}

/// Where each audio frame starts and how many bytes of main data it holds.
#[derive(Debug, Clone, Copy)]
struct Entry {
    offset: u64,
    main_data: u16,
}

struct Mp3Demuxer {
    framer: Framer,
    streams: [StreamInfo; 1],
    tags: Tags,
    first_pts: i64,
    spf: i64,
    channels: u8,
    /// The first audio frame and its bytes, found while opening and not yet returned.
    pending: Option<(Frame, Vec<u8>)>,
    /// Audio frames returned so far (the next one's number).
    packets: i64,
    /// Where the first audio frame starts (after any info frame).
    audio_start: u64,
    /// Every audio frame, built by one scan of a seekable input when first needed.
    index: Option<Vec<Entry>>,
    max_alloc: usize,
    max_packet: usize,
    done: bool,
}

impl Mp3Demuxer {
    /// Scans the whole input once for frame headers (reading in large chunks, decoding
    /// nothing), then goes back to where reading was.
    fn build_index(&mut self) -> Result<&[Entry], Error> {
        if self.index.is_none() {
            let resume = self.framer.position();
            let resume_synced = self.pending.is_none();
            self.framer.reposition(self.audio_start, true)?;
            let mut index = Vec::new();
            while let Ok(f) = self.framer.next()? {
                let requested = (index.len() + 1) * size_of::<Entry>();
                if requested > self.max_alloc {
                    return Err(Error::AllocLimitExceeded {
                        requested,
                        max: self.max_alloc,
                    });
                }
                let h = f.header;
                let main = f.len.saturating_sub(h.data_start() + h.side_info_len());
                index.push(Entry {
                    offset: f.offset,
                    main_data: u16::try_from(main).unwrap_or(u16::MAX),
                });
            }
            self.framer.reposition(resume, resume_synced)?;
            self.index = Some(index);
        }
        Ok(self.index.as_deref().unwrap_or_default())
    }

    fn malformed_change(&self, offset: u64, what: &str) -> Error {
        Error::Unsupported {
            what: format!(
                "mp3: the {what} changes at byte {offset}, which this reader cannot follow"
            ),
        }
    }
}

impl Demuxer for Mp3Demuxer {
    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }

    fn tags(&self) -> &Tags {
        &self.tags
    }

    fn read_packet(&mut self, out: &mut Packet) -> Result<bool, Error> {
        if self.done {
            return Ok(false);
        }
        let pending = self.pending.take();
        let frame = match &pending {
            Some((f, _)) => *f,
            None => match self.framer.next()? {
                Ok(f) => f,
                Err(Stop::End) => {
                    self.done = true;
                    return Ok(false);
                }
                Err(Stop::Changed { offset, header }) => {
                    let s = self.framer.stream().unwrap_or(header);
                    let what = if s.sample_rate != header.sample_rate {
                        "sample rate"
                    } else {
                        "MPEG version or layer"
                    };
                    return Err(self.malformed_change(offset, what));
                }
            },
        };
        if frame.header.channels() != self.channels {
            return Err(self.malformed_change(frame.offset, "channel count"));
        }
        if frame.len > self.max_packet {
            return Err(Error::Malformed {
                what: format!("mp3: frame of {} bytes exceeds the packet limit", frame.len),
            });
        }
        out.stream = 0;
        out.pts = self.first_pts + self.packets * self.spf;
        out.duration = self.spf as u64;
        out.data.clear();
        match &pending {
            Some((_, bytes)) => out.data.extend_from_slice(bytes),
            None => out.data.extend_from_slice(self.framer.bytes(&frame)),
        }
        self.packets += 1;
        Ok(true)
    }

    fn seek(&mut self, pts: i64) -> Result<i64, Error> {
        if !self.framer.source().is_seekable() {
            return Err(Error::Unsupported {
                what: "mp3: cannot seek: the input is not seekable".into(),
            });
        }
        let spf = self.spf;
        let first_pts = self.first_pts;
        let frames = self.streams[0].frames;
        let Some(stream) = self.framer.stream() else {
            return Ok(first_pts);
        };
        let lsf = stream.version.is_lsf();
        let index = self.build_index()?;
        let total = index.len() as i64;
        let end_pts = frames.map_or(first_pts + total * spf, |f| f as i64);
        if pts >= end_pts || total == 0 {
            self.pending = None;
            self.done = true;
            self.packets = total;
            return Ok(end_pts.max(first_pts));
        }
        let target = ((pts.max(first_pts) - first_pts) / spf).min(total - 1);
        let j = first_frame_to_decode(target as usize, lsf, |i| usize::from(index[i].main_data));
        let offset = index[j].offset;
        self.framer.reposition(offset, true)?;
        self.pending = None;
        self.done = false;
        self.packets = j as i64;
        Ok(first_pts + j as i64 * spf)
    }
}

#[cfg(test)]
mod tests;
