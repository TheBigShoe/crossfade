use super::FRAMES_PER_PACKET;
use super::INFO_CAP;
use crate::aiff::decode_text;
use crossfade_core::{
    AudioSpec, ChannelLayout, CodecId, Demuxer, Error, Limits, MediaSource, Packet, SampleFormat,
    StreamInfo, Tags, TimeBase, tag_keys,
};
use std::io::{ErrorKind, SeekFrom};

const STREAMING: u32 = 0xFFFF_FFFF;
const FORMAT_PCM: u16 = 1;
const FORMAT_FLOAT: u16 = 3;
const FORMAT_EXTENSIBLE: u16 = 0xFFFE;
/// Bytes of a `fmt ` chunk that carry fields (the extensible layout).
const FMT_FIELDS: usize = 40;

pub struct WavDemuxer {
    source: Box<dyn MediaSource>,
    pos: u64,
    streams: [StreamInfo; 1],
    tags: Tags,
    bytes_per_frame: usize,
    frames_per_packet: usize,
    /// Where the first frame is.
    data_start: u64,
    /// The header's frame count; `None` for a streaming file.
    frames: Option<u64>,
    next_frame: u64,
    /// `None` for a streaming file: read until the input ends.
    frames_left: Option<u64>,
    /// A truncated file still yields the whole frames that were there; the
    /// error follows on the next call.
    pending_eof: Option<u64>,
    done: bool,
}

fn malformed(what: &str) -> Error {
    Error::Malformed {
        what: what.to_owned(),
    }
}

fn unsupported(what: String) -> Error {
    Error::Unsupported { what }
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// Fills `buf` unless the source ends first; returns the bytes read.
pub(crate) fn fill(
    source: &mut dyn MediaSource,
    pos: &mut u64,
    buf: &mut [u8],
) -> Result<usize, Error> {
    let mut n = 0;
    while n < buf.len() {
        match source.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k.min(buf.len() - n),
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    *pos = pos.saturating_add(n as u64);
    Ok(n)
}

pub(crate) fn fill_exact(
    source: &mut dyn MediaSource,
    pos: &mut u64,
    buf: &mut [u8],
) -> Result<(), Error> {
    if fill(source, pos, buf)? < buf.len() {
        return Err(Error::UnexpectedEof { offset: *pos });
    }
    Ok(())
}

/// Skips by reading: the source may be a pipe.
pub(crate) fn skip(source: &mut dyn MediaSource, pos: &mut u64, mut len: u64) -> Result<(), Error> {
    let mut scratch = [0u8; 4096];
    while len > 0 {
        let step = scratch
            .len()
            .min(usize::try_from(len).unwrap_or(usize::MAX));
        fill_exact(source, pos, &mut scratch[..step])?;
        len -= step as u64;
    }
    Ok(())
}

/// What every reader says when asked to seek a source that cannot.
pub(crate) fn not_seekable() -> Error {
    Error::Unsupported {
        what: "this input cannot seek".into(),
    }
}

/// Where the reader's offset 0 lies in the source: a reader counts `pos` from where it started
/// reading, which need not be the source's start.
pub(crate) fn origin(source: &mut dyn MediaSource, pos: u64) -> Result<u64, Error> {
    if !source.is_seekable() {
        return Err(not_seekable());
    }
    let here = source.seek(SeekFrom::Current(0))?;
    here.checked_sub(pos).ok_or(Error::Overflow)
}

/// The source's length counted like `pos`.
pub(crate) fn source_len(source: &mut dyn MediaSource, origin: u64) -> Result<u64, Error> {
    let len = match source.byte_len() {
        Some(len) => len,
        None => source.seek(SeekFrom::End(0))?,
    };
    Ok(len.saturating_sub(origin))
}

/// Moves to `offset`, counted like `pos`.
pub(crate) fn seek_to(
    source: &mut dyn MediaSource,
    origin: u64,
    pos: &mut u64,
    offset: u64,
) -> Result<(), Error> {
    let at = origin.checked_add(offset).ok_or(Error::Overflow)?;
    source.seek(SeekFrom::Start(at))?;
    *pos = offset;
    Ok(())
}

fn tag_name(what: &str, tag: u16) -> Error {
    unsupported(format!("{what} {tag} ({tag:#x})"))
}

/// The stream's format and, for an extensible header with fewer valid bits than its container
/// holds, the true depth (samples are left-justified in the container).
fn parse_fmt(f: &[u8]) -> Result<(AudioSpec, Option<u8>), Error> {
    let mut tag = u16_at(f, 0);
    let channels = u16_at(f, 2);
    let rate = u32_at(f, 4);
    let block_align = u16_at(f, 12);
    let bits = u16_at(f, 14);
    if channels == 0 {
        return Err(malformed("zero channels"));
    }
    if rate == 0 {
        return Err(malformed("zero sample rate"));
    }
    let mut mask = None;
    let mut raw = None;
    if tag == FORMAT_EXTENSIBLE {
        if f.len() < 40 || u16_at(f, 16) < 22 {
            return Err(malformed("truncated WAVE_FORMAT_EXTENSIBLE header"));
        }
        let valid = u16_at(f, 18);
        if valid == 0 || valid > bits {
            return Err(unsupported(format!(
                "{valid} valid bits in a {bits}-bit container"
            )));
        }
        raw = (valid < bits).then_some(valid);
        mask = Some(u32_at(f, 20));
        tag = u16_at(f, 24);
        if !matches!(tag, FORMAT_PCM | FORMAT_FLOAT) {
            return Err(tag_name("sub-format tag", tag));
        }
    }
    let format = match (tag, bits) {
        (FORMAT_PCM, 8) => SampleFormat::U8,
        (FORMAT_PCM, 16) => SampleFormat::S16,
        (FORMAT_PCM, 24) => SampleFormat::S24,
        (FORMAT_PCM, 32) => SampleFormat::S32,
        (FORMAT_FLOAT, 32) => SampleFormat::F32,
        (FORMAT_FLOAT, 64) => SampleFormat::F64,
        (FORMAT_PCM, _) => return Err(unsupported(format!("{bits}-bit integer PCM"))),
        (FORMAT_FLOAT, _) => return Err(unsupported(format!("{bits}-bit float"))),
        _ => return Err(tag_name("format tag", tag)),
    };
    if raw.is_some() && format.is_float() {
        return Err(unsupported(format!(
            "{} valid bits in a {bits}-bit float container",
            raw.unwrap_or_default()
        )));
    }
    if usize::from(block_align) != usize::from(channels) * format.bytes_per_sample() {
        return Err(malformed("block align does not match channels and depth"));
    }
    let unpositioned = || ChannelLayout::unpositioned(channels);
    let layout = match (mask, channels) {
        (Some(m), _) => match ChannelLayout::from_mask(m, channels) {
            Some(l) => l,
            None => unpositioned()?,
        },
        (None, 1) => ChannelLayout::mono(),
        (None, 2) => ChannelLayout::stereo(),
        (None, _) => unpositioned()?,
    };
    // Valid bits never exceed 32 here: the container is at most 32 bits for integers.
    let raw = raw.and_then(|v| u8::try_from(v).ok());
    Ok((AudioSpec::new(rate, format, layout)?, raw))
}

fn info_key(id: &[u8]) -> Option<&'static str> {
    Some(match id {
        b"INAM" => tag_keys::TITLE,
        b"IART" => tag_keys::ARTIST,
        b"IPRD" => tag_keys::ALBUM,
        b"ICRD" => tag_keys::DATE,
        b"ICMT" => tag_keys::COMMENT,
        b"IGNR" => tag_keys::GENRE,
        b"ITRK" => tag_keys::TRACKNUMBER,
        b"ICOP" => tag_keys::COPYRIGHT,
        _ => return None,
    })
}

/// Values are NUL-terminated; a sub-chunk that overruns the list ends it.
fn parse_info(body: &[u8], tags: &mut Tags) {
    let mut i = 0;
    while i + 8 <= body.len() {
        let size = u32_at(body, i + 4) as usize;
        let start = i + 8;
        let Some(end) = start.checked_add(size).filter(|&e| e <= body.len()) else {
            return;
        };
        if let Some(key) = info_key(&body[i..i + 4]) {
            let value = &body[start..end];
            let value = value.split(|&b| b == 0).next().unwrap_or_default();
            tags.push(key, decode_text(value));
        }
        i = end + size % 2;
    }
}

/// Consumes the whole `LIST` chunk body (and its pad byte). Only `INFO` lists within the
/// caps are read; the declared size is checked before anything is allocated.
fn read_list(
    source: &mut dyn MediaSource,
    pos: &mut u64,
    size: u32,
    limits: &Limits,
    tags: &mut Tags,
    tag_bytes: &mut usize,
) -> Result<(), Error> {
    let padded = u64::from(size) + u64::from(size % 2);
    let cap = INFO_CAP.min(limits.max_alloc_bytes as u64);
    let fits = u64::from(size) <= cap
        && size >= 4
        && (*tag_bytes as u64).saturating_add(u64::from(size)) <= limits.max_alloc_bytes as u64;
    if !fits {
        return skip(source, pos, padded);
    }
    let mut body = vec![0u8; size as usize];
    fill_exact(source, pos, &mut body)?;
    skip(source, pos, padded - u64::from(size))?;
    if body[..4] == *b"INFO" {
        *tag_bytes += body.len();
        parse_info(&body[4..], tags);
    }
    Ok(())
}

/// Reads the chunks after the audio, for a source that can seek past it and back. Whatever
/// goes wrong out there is not an error for the audio.
fn tags_after_data(
    source: &mut dyn MediaSource,
    pos: &mut u64,
    data_size: u32,
    limits: &Limits,
    tags: &mut Tags,
    tag_bytes: &mut usize,
) -> Result<(), Error> {
    let start = *pos;
    let mut walk = || -> Result<(), Error> {
        let mut p = source.seek(SeekFrom::Start(
            start + u64::from(data_size) + u64::from(data_size % 2),
        ))?;
        loop {
            let mut ch = [0u8; 8];
            if fill(source, &mut p, &mut ch)? < 8 {
                return Ok(());
            }
            let size = u32_at(&ch, 4);
            if ch[..4] == *b"LIST" {
                read_list(source, &mut p, size, limits, tags, tag_bytes)?;
            } else {
                p = source.seek(SeekFrom::Start(p + u64::from(size) + u64::from(size % 2)))?;
            }
        }
    };
    let _ = walk();
    *pos = source.seek(SeekFrom::Start(start))?;
    Ok(())
}

impl WavDemuxer {
    pub fn new(mut source: Box<dyn MediaSource>, limits: &Limits) -> Result<Self, Error> {
        let mut pos = 0u64;
        let mut head = [0u8; 12];
        let n = fill(source.as_mut(), &mut pos, &mut head)?;
        if n >= 4 && (head[..4] == *b"RF64" || head[..4] == *b"BW64") {
            return Err(unsupported("RF64/BW64 files".into()));
        }
        if n < 12 || head[..4] != *b"RIFF" || head[8..] != *b"WAVE" {
            return Err(Error::UnrecognisedFormat);
        }
        let seekable = source.is_seekable();
        let mut tags = Tags::new();
        let mut tag_bytes = 0usize;
        let mut fmt: Option<([u8; FMT_FIELDS], usize)> = None;
        loop {
            let mut ch = [0u8; 8];
            match fill(source.as_mut(), &mut pos, &mut ch)? {
                0 => return Err(malformed("no data chunk")),
                8 => {}
                _ => return Err(Error::UnexpectedEof { offset: pos }),
            }
            let size = u32_at(&ch, 4);
            let padded = u64::from(size) + u64::from(size % 2);
            if ch[..4] == *b"data" {
                let (f, len) = fmt.ok_or_else(|| malformed("data chunk before fmt chunk"))?;
                let (audio, raw) = parse_fmt(&f[..len])?;
                if seekable && size != STREAMING {
                    let (src, p) = (source.as_mut(), &mut pos);
                    tags_after_data(src, p, size, limits, &mut tags, &mut tag_bytes)?;
                }
                return Self::start(source, pos, size, (&audio, raw), limits, tags);
            }
            if ch[..4] == *b"LIST" {
                let (src, p) = (source.as_mut(), &mut pos);
                read_list(src, p, size, limits, &mut tags, &mut tag_bytes)?;
                continue;
            }
            if ch[..4] == *b"fmt " && fmt.is_none() {
                // Read only the fields and skip the rest, so a hostile size
                // costs no memory.
                let len = usize::try_from(size).map_or(FMT_FIELDS, |s| s.min(FMT_FIELDS));
                if len < 16 {
                    return Err(malformed("fmt chunk shorter than 16 bytes"));
                }
                let mut body = [0u8; FMT_FIELDS];
                fill_exact(source.as_mut(), &mut pos, &mut body[..len])?;
                skip(source.as_mut(), &mut pos, padded - len as u64)?;
                fmt = Some((body, len));
            } else {
                skip(source.as_mut(), &mut pos, padded)?;
            }
        }
    }

    fn start(
        source: Box<dyn MediaSource>,
        pos: u64,
        data_size: u32,
        (audio, raw): (&AudioSpec, Option<u8>),
        limits: &Limits,
        tags: Tags,
    ) -> Result<Self, Error> {
        let bytes_per_frame = audio.bytes_per_frame();
        if bytes_per_frame > limits.max_packet_bytes {
            return Err(Error::AllocLimitExceeded {
                requested: bytes_per_frame,
                max: limits.max_packet_bytes,
            });
        }
        let frames_per_packet = FRAMES_PER_PACKET.min(limits.max_packet_bytes / bytes_per_frame);
        limits.check_alloc(frames_per_packet * bytes_per_frame)?;
        let frames =
            (data_size != STREAMING).then(|| u64::from(data_size) / bytes_per_frame as u64);
        let rate = audio.sample_rate();
        let format = audio.format();
        let stream = StreamInfo {
            frames,
            bits_per_raw_sample: raw,
            ..StreamInfo::new(
                0,
                CodecId::pcm_for(format),
                audio.clone(),
                TimeBase::new(1, i64::from(rate))?,
            )
        };
        Ok(Self {
            source,
            pos,
            streams: [stream],
            tags,
            bytes_per_frame,
            frames_per_packet,
            data_start: pos,
            frames,
            next_frame: 0,
            frames_left: frames,
            pending_eof: None,
            done: false,
        })
    }
}

impl Demuxer for WavDemuxer {
    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }

    fn tags(&self) -> &Tags {
        &self.tags
    }

    fn read_packet(&mut self, out: &mut Packet) -> Result<bool, Error> {
        if let Some(offset) = self.pending_eof.take() {
            self.done = true;
            return Err(Error::UnexpectedEof { offset });
        }
        if self.done {
            return Ok(false);
        }
        let want = match self.frames_left {
            Some(0) => {
                self.done = true;
                return Ok(false);
            }
            Some(left) => usize::try_from(left)
                .map_or(self.frames_per_packet, |l| l.min(self.frames_per_packet)),
            None => self.frames_per_packet,
        };
        out.data.clear();
        out.data.resize(want * self.bytes_per_frame, 0);
        let got = fill(self.source.as_mut(), &mut self.pos, &mut out.data)?;
        let frames = got / self.bytes_per_frame;
        out.data.truncate(frames * self.bytes_per_frame);
        if frames < want {
            if self.frames_left.is_some() {
                self.pending_eof = Some(self.pos);
            }
            self.done = true;
            if frames == 0 {
                return match self.pending_eof.take() {
                    Some(offset) => Err(Error::UnexpectedEof { offset }),
                    None => Ok(false),
                };
            }
        }
        let n = frames as u64;
        out.stream = 0;
        out.pts = i64::try_from(self.next_frame).map_err(|_| Error::Overflow)?;
        out.duration = n;
        self.next_frame += n;
        if let Some(left) = &mut self.frames_left {
            *left -= n;
        }
        Ok(true)
    }

    /// Exact: PCM has no pre-roll. A streaming file's end is where its source ends.
    fn seek(&mut self, pts: i64) -> Result<i64, Error> {
        let origin = origin(self.source.as_mut(), self.pos)?;
        let bpf = self.bytes_per_frame as u64;
        let total = match self.frames {
            Some(n) => n,
            None => source_len(self.source.as_mut(), origin)?.saturating_sub(self.data_start) / bpf,
        };
        let target = u64::try_from(pts).unwrap_or(0).min(total);
        let offset = self.data_start + target * bpf;
        seek_to(self.source.as_mut(), origin, &mut self.pos, offset)?;
        self.next_frame = target;
        self.frames_left = self.frames.map(|n| n - target);
        self.pending_eof = None;
        self.done = self.frames.is_none() && target == total;
        i64::try_from(target).map_err(|_| Error::Overflow)
    }
}
