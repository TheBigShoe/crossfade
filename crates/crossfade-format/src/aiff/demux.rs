use super::{FRAMES_PER_PACKET, TEXT_CAP, extended};
use crate::wav::{fill, fill_exact, skip};
use crossfade_core::{
    AudioSpec, ChannelLayout, CodecId, Demuxer, Error, Limits, MediaSource, Packet, StreamInfo,
    Tags, TimeBase, tag_keys,
};
use std::io::SeekFrom;

/// `COMM` fields: 18 bytes, plus the 4-byte compression type for AIFF-C.
const COMM_PLAIN: usize = 18;
const COMM_AIFC: usize = 22;

pub struct AiffDemuxer {
    source: Box<dyn MediaSource>,
    pos: u64,
    streams: [StreamInfo; 1],
    tags: Tags,
    bytes_per_frame: usize,
    frames_per_packet: usize,
    next_frame: u64,
    frames_left: u64,
    /// A short file still yields the whole frames that were there; the error follows.
    pending_eof: Option<u64>,
    done: bool,
}

struct Comm {
    channels: u16,
    frames: u32,
    bits: u16,
    rate: u32,
    compression: [u8; 4],
}

fn malformed(what: &str) -> Error {
    Error::Malformed {
        what: what.to_owned(),
    }
}

fn unsupported(what: String) -> Error {
    Error::Unsupported { what }
}

/// UTF-8 when valid, else Latin-1 (every byte a code point, so nothing is lost);
/// trailing NULs trimmed.
pub(crate) fn decode_text(bytes: &[u8]) -> String {
    let end = bytes.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
    let bytes = &bytes[..end];
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => bytes.iter().map(|&b| char::from(b)).collect(),
    }
}

fn parse_comm(body: &[u8], aifc: bool) -> Result<Comm, Error> {
    let need = if aifc { COMM_AIFC } else { COMM_PLAIN };
    if body.len() < need {
        return Err(malformed("COMM chunk too short"));
    }
    let channels = i16::from_be_bytes([body[0], body[1]]);
    let frames = u32::from_be_bytes([body[2], body[3], body[4], body[5]]);
    let bits = i16::from_be_bytes([body[6], body[7]]);
    let mut ext = [0u8; 10];
    ext.copy_from_slice(&body[8..18]);
    let compression = if aifc {
        [body[18], body[19], body[20], body[21]]
    } else {
        *b"NONE"
    };
    let channels = u16::try_from(channels)
        .ok()
        .filter(|&c| c > 0)
        .ok_or_else(|| malformed("zero channels"))?;
    let bits = u16::try_from(bits)
        .ok()
        .filter(|&b| b > 0)
        .ok_or_else(|| malformed("zero sample size"))?;
    Ok(Comm {
        channels,
        frames,
        bits,
        rate: extended::decode(&ext)?,
        compression,
    })
}

/// The codec and, for a reduced depth, the significant bits.
fn codec_for(c: &Comm) -> Result<(CodecId, Option<u8>), Error> {
    let bits = c.bits;
    let integer = |little: bool| {
        if bits > 32 {
            return Err(unsupported(format!("{bits}-bit integer samples")));
        }
        let codec = match (bits.div_ceil(8), little) {
            (1, _) => CodecId::PcmS8,
            (2, false) => CodecId::PcmS16Be,
            (2, true) => CodecId::PcmS16Le,
            (3, false) => CodecId::PcmS24Be,
            (3, true) => CodecId::PcmS24Le,
            (_, false) => CodecId::PcmS32Be,
            (_, true) => CodecId::PcmS32Le,
        };
        let raw = (![8, 16, 24, 32].contains(&bits)).then_some(bits as u8);
        Ok((codec, raw))
    };
    let float = |want: u16, codec| {
        if bits == want {
            Ok((codec, None))
        } else {
            Err(unsupported(format!(
                "{bits}-bit samples in a {want}-bit float type"
            )))
        }
    };
    match &c.compression {
        b"NONE" | b"twos" | b"in24" | b"in32" => integer(false),
        b"sowt" => integer(true),
        b"raw " if bits <= 8 => Ok((CodecId::PcmU8, (bits != 8).then_some(bits as u8))),
        b"raw " => Err(unsupported(format!("{bits}-bit unsigned samples"))),
        b"fl32" | b"FL32" => float(32, CodecId::PcmF32Be),
        b"fl64" | b"FL64" => float(64, CodecId::PcmF64Be),
        other => Err(unsupported(format!(
            "AIFF-C compression type {}",
            String::from_utf8_lossy(other)
        ))),
    }
}

/// Moves forward `len` bytes: a seek where possible, so a hostile size costs nothing.
fn advance(
    source: &mut dyn MediaSource,
    pos: &mut u64,
    seekable: bool,
    len: u64,
) -> Result<(), Error> {
    if seekable {
        *pos = source.seek(SeekFrom::Start(pos.saturating_add(len)))?;
        Ok(())
    } else {
        skip(source, pos, len)
    }
}

fn text_key(id: &[u8; 4]) -> Option<&'static str> {
    Some(match id {
        b"NAME" => tag_keys::TITLE,
        b"AUTH" => tag_keys::ARTIST,
        b"(c) " => tag_keys::COPYRIGHT,
        b"ANNO" => tag_keys::COMMENT,
        _ => return None,
    })
}

impl AiffDemuxer {
    pub fn new(mut source: Box<dyn MediaSource>, limits: &Limits) -> Result<Self, Error> {
        let seekable = source.is_seekable();
        let mut pos = 0u64;
        let mut head = [0u8; 12];
        let n = fill(source.as_mut(), &mut pos, &mut head)?;
        if n < 12 || head[..4] != *b"FORM" || (head[8..] != *b"AIFF" && head[8..] != *b"AIFC") {
            return Err(Error::UnrecognisedFormat);
        }
        let aifc = head[8..] == *b"AIFC";
        // Writers that stream leave this wrong; only trust it when it fits the input.
        let form_end = source.byte_len().and_then(|len| {
            let end = 8 + u64::from(u32::from_be_bytes([head[4], head[5], head[6], head[7]]));
            (12..=len).contains(&end).then_some(end)
        });

        let mut comm: Option<Comm> = None;
        // Where the first frame is, once SSND has been seen.
        let mut sound: Option<u64> = None;
        let mut tags = Tags::new();
        let mut tag_bytes = 0usize;
        loop {
            let have_both = comm.is_some() && sound.is_some();
            if form_end.is_some_and(|end| pos + 8 > end) {
                break;
            }
            let mut h = [0u8; 8];
            match fill(source.as_mut(), &mut pos, &mut h)? {
                0 => break,
                8 => {}
                _ if have_both => break,
                _ => return Err(Error::UnexpectedEof { offset: pos }),
            }
            let id = [h[0], h[1], h[2], h[3]];
            let Ok(size) = u64::try_from(i32::from_be_bytes([h[4], h[5], h[6], h[7]])) else {
                if have_both {
                    break;
                }
                return Err(malformed("negative chunk size"));
            };
            let padded = size + size % 2;
            let start = pos;
            match &id {
                b"COMM" if comm.is_none() => {
                    if size < COMM_PLAIN as u64 {
                        return Err(malformed("COMM chunk too short"));
                    }
                    let len = usize::try_from(size).map_or(COMM_AIFC, |s| s.min(COMM_AIFC));
                    let mut body = [0u8; COMM_AIFC];
                    fill_exact(source.as_mut(), &mut pos, &mut body[..len])?;
                    advance(source.as_mut(), &mut pos, seekable, padded - len as u64)?;
                    comm = Some(parse_comm(&body[..len], aifc)?);
                }
                b"SSND" if sound.is_none() => {
                    if size < 8 {
                        return Err(malformed("SSND chunk too short"));
                    }
                    let mut f = [0u8; 8];
                    fill_exact(source.as_mut(), &mut pos, &mut f)?;
                    let offset = u64::from(u32::from_be_bytes([f[0], f[1], f[2], f[3]]));
                    if seekable {
                        sound = Some(pos + offset);
                        pos = source.seek(SeekFrom::Start(start + padded))?;
                    } else {
                        if comm.is_none() {
                            return Err(unsupported(
                                "AIFF with sound data before its COMM chunk needs a seekable input"
                                    .into(),
                            ));
                        }
                        skip(source.as_mut(), &mut pos, offset)?;
                        sound = Some(pos);
                        break;
                    }
                }
                _ => match text_key(&id) {
                    Some(key)
                        if size <= TEXT_CAP.min(limits.max_alloc_bytes as u64)
                            && tag_bytes + size as usize <= limits.max_alloc_bytes =>
                    {
                        let mut text = vec![0u8; size as usize];
                        fill_exact(source.as_mut(), &mut pos, &mut text)?;
                        advance(source.as_mut(), &mut pos, seekable, size % 2)?;
                        tag_bytes += text.len();
                        tags.push(key, decode_text(&text));
                    }
                    _ => advance(source.as_mut(), &mut pos, seekable, padded)?,
                },
            }
        }
        let comm = comm.ok_or_else(|| malformed("no COMM chunk"))?;
        let sound = sound.ok_or_else(|| malformed("no SSND chunk"))?;
        if seekable {
            pos = source.seek(SeekFrom::Start(sound))?;
        }
        Self::start(source, pos, &comm, tags, limits)
    }

    fn start(
        source: Box<dyn MediaSource>,
        pos: u64,
        comm: &Comm,
        tags: Tags,
        limits: &Limits,
    ) -> Result<Self, Error> {
        let (codec, raw_bits) = codec_for(comm)?;
        let format = codec
            .pcm_sample_format()
            .ok_or_else(|| malformed("not a PCM codec"))?;
        let layout = match comm.channels {
            1 => ChannelLayout::mono(),
            2 => ChannelLayout::stereo(),
            n => ChannelLayout::unpositioned(n)?,
        };
        let audio = AudioSpec::new(comm.rate, format, layout)?;
        let bytes_per_frame = audio.bytes_per_frame();
        if bytes_per_frame > limits.max_packet_bytes {
            return Err(Error::AllocLimitExceeded {
                requested: bytes_per_frame,
                max: limits.max_packet_bytes,
            });
        }
        let frames_per_packet = FRAMES_PER_PACKET.min(limits.max_packet_bytes / bytes_per_frame);
        limits.check_alloc(frames_per_packet * bytes_per_frame)?;
        let stream = StreamInfo {
            frames: Some(u64::from(comm.frames)),
            bits_per_raw_sample: raw_bits,
            ..StreamInfo::new(0, codec, audio, TimeBase::new(1, i64::from(comm.rate))?)
        };
        Ok(Self {
            source,
            pos,
            streams: [stream],
            tags,
            bytes_per_frame,
            frames_per_packet,
            next_frame: 0,
            frames_left: u64::from(comm.frames),
            pending_eof: None,
            done: false,
        })
    }
}

impl Demuxer for AiffDemuxer {
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
        if self.done || self.frames_left == 0 {
            self.done = true;
            return Ok(false);
        }
        let want = usize::try_from(self.frames_left)
            .map_or(self.frames_per_packet, |l| l.min(self.frames_per_packet));
        out.data.clear();
        out.data.resize(want * self.bytes_per_frame, 0);
        let got = fill(self.source.as_mut(), &mut self.pos, &mut out.data)?;
        let frames = got / self.bytes_per_frame;
        out.data.truncate(frames * self.bytes_per_frame);
        if frames < want {
            self.done = true;
            if frames == 0 {
                return Err(Error::UnexpectedEof { offset: self.pos });
            }
            self.pending_eof = Some(self.pos);
        }
        let n = frames as u64;
        out.stream = 0;
        out.pts = i64::try_from(self.next_frame).map_err(|_| Error::Overflow)?;
        out.duration = n;
        self.next_frame += n;
        self.frames_left -= n;
        Ok(true)
    }
}
