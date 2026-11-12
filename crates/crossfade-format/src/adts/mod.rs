//! AAC in ADTS frames (`.aac`): every frame is a header (`crossfade_aac::adts`) and one to
//! four raw data blocks; each block is a packet of 1024 frames.
//!
//! - The first header is confirmed by a second one of the same stream where its length says,
//!   or by the input ending exactly there: an MPEG-2 sync word alone can be a FLAC frame header.
//! - The CRC is skipped, not verified: it covers the header and the first 192 bits of each
//!   channel element, which only a bitstream parse can find, and a bad frame is the decoder's
//!   to report.
//! - Garbage between frames is skipped (at most [`MAX_RESYNC`] bytes in a row); a header found
//!   after garbage must be followed by another one (or the end) to count. A truncated last frame,
//!   and a trailing tag, are dropped.
//! - ADTS carries no priming information, so packets count 1024 frames each from pts 0 and
//!   `StreamInfo::frames` (known when the source can seek and the frames were indexed once) is
//!   every decoded frame, the encoder's priming and padding included.
//! - A frame of several blocks is split at the positions its CRC-protected header gives; without
//!   a CRC it cannot be split without decoding, and is refused by name.

use crossfade_aac::adts::Header;
use crossfade_aac::asc::ProgramConfig;
use crossfade_core::{
    AudioSpec, ChannelLayout, CodecId, Demuxer, Error, Limits, MediaSource, Packet, SampleFormat,
    StreamInfo, TimeBase,
};
use std::io::SeekFrom;

/// Garbage the reader skips looking for the next frame before it gives up.
pub(crate) const MAX_RESYNC: usize = 64 << 10;
/// Seek index entries at most; the stride between entries doubles to stay under it.
pub(crate) const MAX_INDEX: usize = 4096;

const FRAME_LEN: i64 = 1024;
const CHUNK: usize = 64 << 10;

/// Opens an ADTS stream. `source` is positioned at the first frame: ID3v2 tags in front of it
/// were consumed while sniffing (and are dropped), and positions on `source` are offsets in the
/// whole input.
pub fn open(source: Box<dyn MediaSource>, limits: &Limits) -> Result<Box<dyn Demuxer>, Error> {
    Ok(Box::new(AdtsReader::open(source, limits)?))
}

/// A read buffer over the source that knows each byte's offset in the input.
struct Input {
    src: Box<dyn MediaSource>,
    buf: Vec<u8>,
    start: usize,
    end: usize,
    /// Input offset of `buf[0]`.
    offset: u64,
    eof: bool,
}

impl Input {
    fn available(&self) -> &[u8] {
        &self.buf[self.start..self.end]
    }

    fn position(&self) -> u64 {
        self.offset + self.start as u64
    }

    /// Makes at least `n` bytes available unless the input ends first.
    fn fill(&mut self, n: usize) -> Result<(), Error> {
        if self.end - self.start >= n || self.eof {
            return Ok(());
        }
        self.buf.copy_within(self.start..self.end, 0);
        self.end -= self.start;
        self.offset += self.start as u64;
        self.start = 0;
        let want = n.max(CHUNK);
        if self.buf.len() < want {
            self.buf.resize(want, 0);
        }
        while self.end < n {
            match self.src.read(&mut self.buf[self.end..]) {
                Ok(0) => {
                    self.eof = true;
                    break;
                }
                Ok(k) => self.end += k,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    fn consume(&mut self, n: usize) {
        self.start += n.min(self.end - self.start);
    }

    fn seek_to(&mut self, at: u64) -> Result<(), Error> {
        self.src.seek(SeekFrom::Start(at))?;
        (self.start, self.end, self.offset, self.eof) = (0, 0, at, false);
        Ok(())
    }
}

/// One entry per `stride` frames: the number of the first block of a frame, and where the
/// frame starts.
struct Index {
    entries: Vec<(u64, u64)>,
    stride: u64,
    frames: u64,
    blocks: u64,
}

impl Index {
    /// Notes the frame starting at `offset` with `blocks` blocks.
    fn push(&mut self, offset: u64, blocks: u8) {
        let (frame, block) = (self.frames, self.blocks);
        self.frames += 1;
        self.blocks += u64::from(blocks);
        if !frame.is_multiple_of(self.stride) {
            return;
        }
        if self.entries.len() == MAX_INDEX {
            self.stride *= 2;
            let mut i = 0;
            self.entries.retain(|_| {
                i += 1;
                (i - 1) % 2 == 0
            });
            if !frame.is_multiple_of(self.stride) {
                return;
            }
        }
        self.entries.push((block, offset));
    }
}

pub(crate) struct AdtsReader {
    input: Input,
    first: Header,
    streams: [StreamInfo; 1],
    /// Blocks handed out so far (or the block the reader stands at after a seek).
    next: u64,
    /// The rest of a frame of several blocks: the frame, and its blocks' ranges still to go.
    pending: Vec<u8>,
    pending_blocks: Vec<(usize, usize)>,
    index: Option<Index>,
}

impl AdtsReader {
    pub(crate) fn open(source: Box<dyn MediaSource>, limits: &Limits) -> Result<Self, Error> {
        limits.check_alloc(CHUNK)?;
        let mut input = Input {
            src: source,
            buf: Vec::new(),
            start: 0,
            end: 0,
            offset: 0,
            eof: false,
        };
        let seekable = input.src.is_seekable();
        if seekable {
            input.offset = input.src.seek(SeekFrom::Current(0))?;
        }
        let data_start = input.offset;
        input.fill(Header::MIN_LEN + 2)?;
        let first = Header::parse(input.available()).map_err(|_| Error::UnrecognisedFormat)?;
        let len = usize::from(first.frame_length);
        input.fill(len + Header::MIN_LEN)?;
        if !confirmed(&input, &first) {
            return Err(Error::UnrecognisedFormat);
        }
        let first_block = first
            .raw_data_blocks(&input.available()[..len])
            .map_err(aac_error)?
            .next()
            .unwrap_or_default();
        let layout = layout(&first, first_block)?;
        let audio = AudioSpec::new(first.sample_rate(), SampleFormat::F32, layout)?;
        let time_base = TimeBase::new(1, i64::from(first.sample_rate()))?;
        let mut stream = StreamInfo::new(0, CodecId::Aac, audio, time_base);
        stream.extradata = first.audio_specific_config().to_vec();
        let mut reader = Self {
            input,
            first,
            streams: [stream],
            next: 0,
            pending: Vec::new(),
            pending_blocks: Vec::new(),
            index: None,
        };
        if seekable {
            let mut index = Index {
                entries: Vec::new(),
                stride: 1,
                frames: 0,
                blocks: 0,
            };
            while let Some(h) = reader.next_frame()? {
                index.push(reader.input.position(), h.raw_data_blocks);
                reader.input.consume(usize::from(h.frame_length));
            }
            reader.streams[0].frames = Some(index.blocks.saturating_mul(FRAME_LEN as u64));
            reader.index = Some(index);
            reader.input.seek_to(data_start)?;
        }
        Ok(reader)
    }

    #[cfg(test)]
    pub(crate) fn index_len(&self) -> usize {
        self.index.as_ref().map_or(0, |i| i.entries.len())
    }

    /// Finds the next frame of the stream and makes all of it available, or `None` at the end.
    fn next_frame(&mut self) -> Result<Option<Header>, Error> {
        let mut skipped = 0;
        loop {
            self.input.fill(Header::MIN_LEN + 2)?;
            if self.input.available().len() < Header::MIN_LEN {
                return Ok(None);
            }
            if let Ok(h) = Header::parse(self.input.available())
                && h.same_stream(&self.first)
            {
                let len = usize::from(h.frame_length);
                self.input.fill(len + Header::MIN_LEN)?;
                if self.input.available().len() < len {
                    // A truncated last frame.
                    return Ok(None);
                }
                if skipped == 0 || confirmed(&self.input, &h) {
                    // Refuses what cannot be split before anything is handed out.
                    h.raw_data_blocks(&self.input.available()[..len])
                        .map_err(aac_error)?;
                    return Ok(Some(h));
                }
            }
            if skipped == MAX_RESYNC {
                return Err(Error::Malformed {
                    what: format!(
                        "adts: no frame header in the {MAX_RESYNC} bytes after offset {}",
                        self.input.position() - skipped as u64
                    ),
                });
            }
            self.input.consume(1);
            skipped += 1;
        }
    }
}

/// Whether a header of the same stream follows the frame `h` starts with, or the input ends
/// where it does.
fn confirmed(input: &Input, h: &Header) -> bool {
    let len = usize::from(h.frame_length);
    let avail = input.available();
    match avail.get(len..) {
        None => false,
        Some(rest) if rest.len() < Header::MIN_LEN => input.eof,
        Some(rest) => Header::parse(rest).is_ok_and(|next| next.same_stream(h)),
    }
}

fn aac_error(e: crossfade_aac::Error) -> Error {
    match e {
        crossfade_aac::Error::Unsupported(what) => Error::Unsupported { what },
        crossfade_aac::Error::UnexpectedEnd => Error::Malformed {
            what: "adts: frame ends early".into(),
        },
        other => Error::Malformed {
            what: other.to_string(),
        },
    }
}

/// The layout of channel configurations 1 to 7 in storage (WAVE mask) order, which is the
/// order the decoder emits; with configuration 0, as many unpositioned channels as the program
/// config element opening the first block names.
fn layout(h: &Header, first_block: &[u8]) -> Result<ChannelLayout, Error> {
    let mask: u32 = match h.channel_configuration {
        0 => {
            let pce = ProgramConfig::from_raw_data_block(first_block)
                .ok()
                .flatten()
                .ok_or_else(|| Error::Unsupported {
                    what: "adts: channel configuration 0 without a program config element first"
                        .into(),
                })?;
            return ChannelLayout::unpositioned(pce.channels());
        }
        1 => return Ok(ChannelLayout::mono()),
        2 => 0x3,
        3 => 0x7,
        4 => 0x107,
        5 => 0x607,
        6 => 0x60F,
        _ => 0x6CF,
    };
    let channels = mask.count_ones() as u16;
    ChannelLayout::from_mask(mask, channels).ok_or(Error::Overflow)
}

impl Demuxer for AdtsReader {
    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }

    fn read_packet(&mut self, out: &mut Packet) -> Result<bool, Error> {
        out.data.clear();
        if self.pending_blocks.is_empty() {
            let Some(h) = self.next_frame()? else {
                return Ok(false);
            };
            let len = usize::from(h.frame_length);
            let frame = &self.input.available()[..len];
            let mut blocks = h.raw_data_blocks(frame).map_err(aac_error)?;
            if h.raw_data_blocks == 1 {
                out.data
                    .extend_from_slice(blocks.next().unwrap_or_default());
            } else {
                self.pending.clear();
                self.pending.extend_from_slice(frame);
                // Kept in reverse, so the next block is popped off the end.
                let base = frame.as_ptr() as usize;
                self.pending_blocks.extend(
                    blocks
                        .map(|b| (b.as_ptr() as usize - base, b.len()))
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev(),
                );
            }
            self.input.consume(len);
        }
        if let Some((start, n)) = self.pending_blocks.pop() {
            out.data.extend_from_slice(&self.pending[start..start + n]);
        }
        out.stream = 0;
        out.pts = i64::try_from(self.next)
            .ok()
            .and_then(|n| n.checked_mul(FRAME_LEN))
            .ok_or(Error::Overflow)?;
        out.duration = FRAME_LEN as u64;
        self.next += 1;
        Ok(true)
    }

    fn seek(&mut self, pts: i64) -> Result<i64, Error> {
        let Some(index) = &self.index else {
            return Err(Error::Unsupported {
                what: "adts: the source cannot seek".into(),
            });
        };
        let total = index.blocks;
        // One packet of pre-roll: the overlap-add needs the previous frame.
        let wanted = (pts.max(0) / FRAME_LEN) as u64;
        let target = if wanted >= total {
            total
        } else {
            wanted.saturating_sub(1)
        };
        let Some(&(mut block, offset)) = index.entries.iter().rev().find(|e| e.0 <= target) else {
            return Ok(0);
        };
        self.input.seek_to(offset)?;
        self.pending_blocks.clear();
        while block < target {
            let Some(h) = self.next_frame()? else {
                break;
            };
            let n = u64::from(h.raw_data_blocks);
            if block + n > target {
                // The target is inside this frame: read it and drop the blocks before.
                self.next = block;
                let mut p = Packet::default();
                for _ in block..target {
                    self.read_packet(&mut p)?;
                }
                block = target;
                break;
            }
            self.input.consume(usize::from(h.frame_length));
            block += n;
        }
        self.next = block;
        Ok(self.next as i64 * FRAME_LEN)
    }
}

#[cfg(test)]
mod tests;
