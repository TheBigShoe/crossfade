use super::FRAMES_PER_PACKET;
use crossfade_core::{
    AudioSpec, Channel, ChannelLayout, CodecId, Demuxer, Error, Limits, MediaSource, Packet,
    SampleFormat, StreamInfo, TimeBase,
};
use std::io::ErrorKind;

const STREAMING: u32 = 0xFFFF_FFFF;
const FORMAT_PCM: u16 = 1;
const FORMAT_FLOAT: u16 = 3;
const FORMAT_EXTENSIBLE: u16 = 0xFFFE;

pub struct WavDemuxer {
    source: Box<dyn MediaSource>,
    pos: u64,
    streams: [StreamInfo; 1],
    bytes_per_frame: usize,
    frames_per_packet: usize,
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
fn fill(source: &mut dyn MediaSource, pos: &mut u64, buf: &mut [u8]) -> Result<usize, Error> {
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

fn fill_exact(source: &mut dyn MediaSource, pos: &mut u64, buf: &mut [u8]) -> Result<(), Error> {
    if fill(source, pos, buf)? < buf.len() {
        return Err(Error::UnexpectedEof { offset: *pos });
    }
    Ok(())
}

/// Skips by reading: the source may be a pipe.
fn skip(source: &mut dyn MediaSource, pos: &mut u64, mut len: u64) -> Result<(), Error> {
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

fn tag_name(what: &str, tag: u16) -> Error {
    unsupported(format!("{what} {tag} ({tag:#x})"))
}

fn parse_fmt(f: &[u8]) -> Result<AudioSpec, Error> {
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
    if tag == FORMAT_EXTENSIBLE {
        if f.len() < 40 || u16_at(f, 16) < 22 {
            return Err(malformed("truncated WAVE_FORMAT_EXTENSIBLE header"));
        }
        let valid = u16_at(f, 18);
        if valid != bits {
            return Err(unsupported(format!(
                "{valid} valid bits in a {bits}-bit container"
            )));
        }
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
    if usize::from(block_align) != usize::from(channels) * format.bytes_per_sample() {
        return Err(malformed("block align does not match channels and depth"));
    }
    let unpositioned = || ChannelLayout::unpositioned(channels);
    let layout = match (mask, channels) {
        (Some(m), _) => match layout_from_mask(m, channels) {
            Some(l) => l,
            None => unpositioned()?,
        },
        (None, 1) => ChannelLayout::mono(),
        (None, 2) => ChannelLayout::stereo(),
        (None, _) => unpositioned()?,
    };
    AudioSpec::new(rate, format, layout)
}

/// Bit order of the WAVE channel mask; `None` unless every set bit is known
/// and their count equals the channel count.
fn layout_from_mask(mask: u32, channels: u16) -> Option<ChannelLayout> {
    const BITS: [(u32, Channel); 8] = [
        (0, Channel::FrontLeft),
        (1, Channel::FrontRight),
        (2, Channel::FrontCenter),
        (3, Channel::Lfe),
        (4, Channel::BackLeft),
        (5, Channel::BackRight),
        (9, Channel::SideLeft),
        (10, Channel::SideRight),
    ];
    let known = BITS.iter().fold(0u32, |m, (b, _)| m | 1 << b);
    if mask & !known != 0 || mask.count_ones() != u32::from(channels) {
        return None;
    }
    let positions = BITS
        .iter()
        .filter(|(b, _)| mask & (1 << b) != 0)
        .map(|&(_, c)| c)
        .collect();
    ChannelLayout::from_positions(positions).ok()
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
        let mut fmt: Option<Vec<u8>> = None;
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
                let f = fmt.ok_or_else(|| malformed("data chunk before fmt chunk"))?;
                return Self::start(source, pos, size, &parse_fmt(&f)?, limits);
            }
            if ch[..4] == *b"fmt " && fmt.is_none() {
                let len = usize::try_from(size).unwrap_or(usize::MAX);
                limits.check_alloc(len)?;
                if len < 16 {
                    return Err(malformed("fmt chunk shorter than 16 bytes"));
                }
                let mut body = vec![0u8; len];
                fill_exact(source.as_mut(), &mut pos, &mut body)?;
                skip(source.as_mut(), &mut pos, padded - u64::from(size))?;
                fmt = Some(body);
            } else {
                skip(source.as_mut(), &mut pos, padded)?;
            }
        }
    }

    fn start(
        source: Box<dyn MediaSource>,
        pos: u64,
        data_size: u32,
        audio: &AudioSpec,
        limits: &Limits,
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
            index: 0,
            codec: CodecId::pcm_for(format),
            audio: audio.clone(),
            time_base: TimeBase::new(1, i64::from(rate))?,
            frames,
        };
        Ok(Self {
            source,
            pos,
            streams: [stream],
            bytes_per_frame,
            frames_per_packet,
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
}
