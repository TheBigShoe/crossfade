//! The builder here is deliberately independent of the muxer.

use super::*;
use crossfade_core::{
    AudioSpec, ChannelLayout, CodecId, Demuxer, Error, Limits, Packet, SampleFormat,
    TimeBase,
};
use std::io::Cursor;

const TAG_PCM: u16 = 1;
const TAG_FLOAT: u16 = 3;

struct Fmt {
    tag: u16,
    channels: u16,
    rate: u32,
    bits: u16,
}

impl Fmt {
    fn new(tag: u16, channels: u16, rate: u32, bits: u16) -> Self {
        Self {
            tag,
            channels,
            rate,
            bits,
        }
    }

    fn block_align(&self) -> u16 {
        self.channels * self.bits / 8
    }

    fn base(&self, tag: u16) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&tag.to_le_bytes());
        b.extend_from_slice(&self.channels.to_le_bytes());
        b.extend_from_slice(&self.rate.to_le_bytes());
        let byte_rate = self.rate.wrapping_mul(u32::from(self.block_align()));
        b.extend_from_slice(&byte_rate.to_le_bytes());
        b.extend_from_slice(&self.block_align().to_le_bytes());
        b.extend_from_slice(&self.bits.to_le_bytes());
        b
    }

    fn body(&self) -> Vec<u8> {
        self.base(self.tag)
    }
}

fn raw_chunk(id: &[u8; 4], declared: u32, body: &[u8]) -> Vec<u8> {
    let mut c = id.to_vec();
    c.extend_from_slice(&declared.to_le_bytes());
    c.extend_from_slice(body);
    c
}

/// Chunk with the RIFF pad byte after an odd body.
fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut c = raw_chunk(id, body.len() as u32, body);
    if body.len() % 2 == 1 {
        c.push(0);
    }
    c
}

fn riff(chunks: &[Vec<u8>]) -> Vec<u8> {
    let body: Vec<u8> = chunks.concat();
    let mut f = b"RIFF".to_vec();
    f.extend_from_slice(&(body.len() as u32 + 4).to_le_bytes());
    f.extend_from_slice(b"WAVE");
    f.extend_from_slice(&body);
    f
}

fn simple(fmt: &Fmt, pcm: &[u8]) -> Vec<u8> {
    riff(&[chunk(b"fmt ", &fmt.body()), chunk(b"data", pcm)])
}

fn open(bytes: Vec<u8>) -> Result<WavDemuxer, Error> {
    WavDemuxer::new(Box::new(Cursor::new(bytes)), &Limits::default())
}

type Got = (i64, u64, Vec<u8>);

/// Every packet up to end of stream or the first error.
fn drain(d: &mut dyn Demuxer) -> (Vec<Got>, Option<Error>) {
    let mut pkt = Packet::default();
    let mut out = Vec::new();
    loop {
        match d.read_packet(&mut pkt) {
            Ok(true) => out.push((pkt.pts, pkt.duration, pkt.data.clone())),
            Ok(false) => return (out, None),
            Err(e) => return (out, Some(e)),
        }
    }
}

fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 + 3) as u8).collect()
}

fn spec(rate: u32, f: SampleFormat, layout: ChannelLayout) -> AudioSpec {
    AudioSpec::new(rate, f, layout).unwrap()
}

#[test]
fn every_supported_format_reports_its_stream_info() {
    let cases = [
        (TAG_PCM, 8, SampleFormat::U8),
        (TAG_PCM, 16, SampleFormat::S16),
        (TAG_PCM, 24, SampleFormat::S24),
        (TAG_PCM, 32, SampleFormat::S32),
        (TAG_FLOAT, 32, SampleFormat::F32),
        (TAG_FLOAT, 64, SampleFormat::F64),
    ];
    for (tag, bits, format) in cases {
        let fmt = Fmt::new(tag, 2, 44100, bits);
        let pcm = payload(10 * usize::from(fmt.block_align()));
        let mut d = open(simple(&fmt, &pcm)).unwrap();
        let s = d.streams();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].index, 0);
        assert_eq!(s[0].codec, CodecId::pcm_for(format));
        assert_eq!(s[0].audio, spec(44100, format, ChannelLayout::stereo()));
        assert_eq!(s[0].time_base, TimeBase::new(1, 44100).unwrap());
        assert_eq!(s[0].frames, Some(10));
        let (packets, err) = drain(&mut d);
        assert_eq!(err, None);
        assert_eq!(packets, vec![(0, 10, pcm)], "{format:?}");
    }
}

#[test]
fn mono_is_front_centre_and_many_channels_are_unpositioned() {
    let d = open(simple(&Fmt::new(TAG_PCM, 1, 8000, 16), &[])).unwrap();
    assert_eq!(d.streams()[0].audio.layout(), &ChannelLayout::mono());
    let d = open(simple(&Fmt::new(TAG_PCM, 3, 8000, 16), &[])).unwrap();
    assert_eq!(
        d.streams()[0].audio.layout(),
        &ChannelLayout::unpositioned(3).unwrap()
    );
}

#[test]
fn long_data_splits_into_packets_with_running_pts() {
    let fmt = Fmt::new(TAG_PCM, 1, 8000, 8);
    let frames = FRAMES_PER_PACKET * 2 + 5;
    let pcm = payload(frames);
    let mut d = open(simple(&fmt, &pcm)).unwrap();
    let (packets, err) = drain(&mut d);
    assert_eq!(err, None);
    let shape: Vec<_> = packets.iter().map(|p| (p.0, p.1)).collect();
    let n = FRAMES_PER_PACKET as i64;
    assert_eq!(shape, [(0, n as u64), (n, n as u64), (2 * n, 5)]);
    let joined: Vec<u8> = packets.into_iter().flat_map(|p| p.2).collect();
    assert_eq!(joined, pcm);
}

#[test]
fn packet_buffer_is_reused() {
    let fmt = Fmt::new(TAG_PCM, 1, 8000, 8);
    let mut d = open(simple(&fmt, &payload(FRAMES_PER_PACKET * 2))).unwrap();
    let mut pkt = Packet::default();
    assert!(d.read_packet(&mut pkt).unwrap());
    let ptr = pkt.data.as_ptr();
    assert!(d.read_packet(&mut pkt).unwrap());
    assert_eq!(pkt.data.as_ptr(), ptr);
    assert!(!d.read_packet(&mut pkt).unwrap());
}
