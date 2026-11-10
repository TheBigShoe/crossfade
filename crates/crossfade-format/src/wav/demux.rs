use super::FRAMES_PER_PACKET;
use crossfade_core::{
    AudioSpec, Channel, ChannelLayout, CodecId, Demuxer, Error, Limits, MediaSource, Packet,
    SampleFormat, StreamInfo, TimeBase,
};
use std::io::ErrorKind;

const STREAMING: u32 = 0xFFFF_FFFF;

pub struct WavDemuxer {
    source: Box<dyn MediaSource>,
    streams: [StreamInfo; 1],
    bytes_per_frame: usize,
    next_frame: u64,
    /// `None` for a streaming file: read until the input ends.
    frames_left: Option<u64>,
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// Fills `buf` unless the source ends first; returns the bytes read.
fn read_full(source: &mut dyn MediaSource, buf: &mut [u8]) -> Result<usize, Error> {
    let mut n = 0;
    while n < buf.len() {
        match source.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(n)
}

fn parse_fmt(f: &[u8]) -> (AudioSpec, u16) {
    let mut tag = u16_at(f, 0);
    let channels = u16_at(f, 2);
    let bits = u16_at(f, 14);
    let mut mask = None;
    if tag == 0xFFFE {
        tag = u16_at(f, 24);
        mask = Some(u32_at(f, 20));
    }
    let format = match (tag, bits) {
        (1, 8) => SampleFormat::U8,
        (1, 16) => SampleFormat::S16,
        (1, 24) => SampleFormat::S24,
        (1, 32) => SampleFormat::S32,
        (3, 32) => SampleFormat::F32,
        _ => SampleFormat::F64,
    };
    let unpositioned = || ChannelLayout::unpositioned(channels).expect("non-zero");
    let layout = match (mask, channels) {
        (Some(m), _) => layout_from_mask(m, channels).unwrap_or_else(unpositioned),
        (None, 1) => ChannelLayout::mono(),
        (None, 2) => ChannelLayout::stereo(),
        (None, _) => unpositioned(),
    };
    (
        AudioSpec::new(u32_at(f, 4), format, layout).expect("non-zero rate"),
        channels,
    )
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
    pub fn new(mut source: Box<dyn MediaSource>, _limits: &Limits) -> Result<Self, Error> {
        let mut head = [0u8; 12];
        read_full(source.as_mut(), &mut head)?;
        let mut fmt: Option<Vec<u8>> = None;
        loop {
            let mut ch = [0u8; 8];
            read_full(source.as_mut(), &mut ch)?;
            let size = u32_at(&ch, 4);
            if ch[..4] == *b"data" {
                let f = fmt.ok_or(Error::UnrecognisedFormat)?;
                let (audio, _) = parse_fmt(&f);
                let bytes_per_frame = audio.bytes_per_frame();
                let frames = (size != STREAMING).then(|| u64::from(size) / bytes_per_frame as u64);
                let rate = audio.sample_rate();
                let format = audio.format();
                let stream = StreamInfo {
                    index: 0,
                    codec: CodecId::pcm_for(format),
                    audio,
                    time_base: TimeBase::new(1, i64::from(rate))?,
                    frames,
                };
                return Ok(Self {
                    source,
                    streams: [stream],
                    bytes_per_frame,
                    next_frame: 0,
                    frames_left: frames,
                });
            }
            // Skip by reading: the source may be a pipe.
            let total = u64::from(size) + u64::from(size % 2);
            let mut body = vec![0u8; total as usize];
            read_full(source.as_mut(), &mut body)?;
            if ch[..4] == *b"fmt " {
                body.truncate(size as usize);
                fmt = Some(body);
            }
        }
    }
}

impl Demuxer for WavDemuxer {
    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }

    fn read_packet(&mut self, out: &mut Packet) -> Result<bool, Error> {
        let frames = match self.frames_left {
            Some(0) => return Ok(false),
            Some(left) => left.min(FRAMES_PER_PACKET as u64) as usize,
            None => FRAMES_PER_PACKET,
        };
        out.data.clear();
        out.data.resize(frames * self.bytes_per_frame, 0);
        let got = read_full(self.source.as_mut(), &mut out.data)?;
        let frames = got / self.bytes_per_frame;
        out.data.truncate(frames * self.bytes_per_frame);
        if frames == 0 {
            return Ok(false);
        }
        out.stream = 0;
        out.pts = self.next_frame as i64;
        out.duration = frames as u64;
        self.next_frame += frames as u64;
        if let Some(left) = &mut self.frames_left {
            *left -= frames as u64;
        }
        Ok(true)
    }
}
