//! Shared by the seeking tests: an in-memory sink, a source that counts what it reads, and
//! packet helpers.
#![allow(dead_code, unreachable_pub)]

use crossfade_core::{Demuxer, Error, Limits, MediaSink, MediaSource, Packet};
use std::io::{Cursor, SeekFrom};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct Shared(Arc<Mutex<Cursor<Vec<u8>>>>);

impl Shared {
    pub fn bytes(&self) -> Vec<u8> {
        self.0.lock().unwrap().get_ref().clone()
    }
}

impl MediaSink for Shared {
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        std::io::Write::write_all(&mut *self.0.lock().unwrap(), buf)
    }
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        std::io::Seek::seek(&mut *self.0.lock().unwrap(), pos)
    }
    fn finish(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Bytes read and seeks made through a source, readable while a demuxer owns it.
#[derive(Clone, Default)]
pub struct Counter {
    pub bytes: Arc<AtomicU64>,
    pub seeks: Arc<AtomicU64>,
}

impl Counter {
    pub fn take(&self) -> (u64, u64) {
        (
            self.bytes.swap(0, Ordering::Relaxed),
            self.seeks.swap(0, Ordering::Relaxed),
        )
    }
}

pub struct Counting {
    inner: Cursor<Vec<u8>>,
    counter: Counter,
}

impl Counting {
    pub fn new(bytes: Vec<u8>) -> (Self, Counter) {
        let counter = Counter::default();
        (
            Self {
                inner: Cursor::new(bytes),
                counter: counter.clone(),
            },
            counter,
        )
    }
}

impl MediaSource for Counting {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = std::io::Read::read(&mut self.inner, buf)?;
        self.counter.bytes.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.counter.seeks.fetch_add(1, Ordering::Relaxed);
        std::io::Seek::seek(&mut self.inner, pos)
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.inner.get_ref().len() as u64)
    }
    fn is_seekable(&self) -> bool {
        true
    }
}

pub fn open_bytes(bytes: &[u8]) -> Box<dyn Demuxer> {
    crossfade_format::open_demuxer(Box::new(Cursor::new(bytes.to_vec())), &Limits::default())
        .unwrap()
        .1
}

pub fn drain(d: &mut dyn Demuxer) -> Result<Vec<Packet>, Error> {
    let mut out = Vec::new();
    loop {
        let mut p = Packet::default();
        if !d.read_packet(&mut p)? {
            return Ok(out);
        }
        out.push(p);
    }
}

/// Deterministic test noise: a 64-bit LCG's top bits.
pub struct Lcg(pub u64);

impl Lcg {
    pub fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

/// Samples of `bits` depth: a slow ramp under noise, so frames neither compress to nothing
/// nor all look alike.
pub fn signal(frames: usize, channels: usize, bits: u8, seed: u64) -> Vec<i32> {
    let mut rng = Lcg(seed);
    let amp = 1i64 << (bits - 2);
    (0..frames * channels)
        .map(|i| {
            let ramp = ((i / channels) as i64 * 37) % (2 * amp) - amp;
            let noise = rng.below(1 << (bits - 3)) as i64 - (1 << (bits - 4));
            (ramp / 2 + noise) as i32
        })
        .collect()
}

/// What goes in front of the frames as a SEEKTABLE.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Table {
    None,
    /// A point every `n` frames, plus a placeholder.
    Every(usize),
    /// Points whose offsets lead nowhere useful.
    Wrong,
}

pub struct Flac {
    pub bytes: Vec<u8>,
    /// Each frame's first sample and size.
    pub frames: Vec<(u64, usize)>,
    pub max_frame: usize,
}

fn block_header(kind: u8, last: bool, len: usize, out: &mut Vec<u8>) {
    out.push(kind | if last { 0x80 } else { 0 });
    out.extend(&(len as u32).to_be_bytes()[1..]);
}

/// A FLAC file of `samples` built frame by frame: fixed blocks of the encoder's size, or
/// variable `blocks` (cycled) with the variable blocking strategy. `total`: STREAMINFO
/// carries the length.
pub fn flac_file(
    samples: &[i32],
    channels: u8,
    bits: u8,
    blocks: Option<&[usize]>,
    table: Table,
    total: bool,
) -> Flac {
    use crossfade_flac::frame::{parse_header, write_header};
    use crossfade_flac::{Encoder, EncoderConfig, crc::crc16};
    let ch = usize::from(channels);
    let config = EncoderConfig {
        level: 0,
        ..EncoderConfig::new(44_100, channels, bits)
    };
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut starts = Vec::new();
    let n = samples.len() / ch;
    match blocks {
        None => {
            let mut enc = Encoder::new(config).unwrap();
            let (mut out, mut ends) = (Vec::new(), Vec::new());
            enc.write_ends(samples, &mut out, &mut ends).unwrap();
            enc.flush_ends(&mut out, &mut ends).unwrap();
            let mut from = 0;
            let block = enc.block_size() as u64;
            for (i, end) in ends.into_iter().enumerate() {
                frames.push(out[from..end].to_vec());
                starts.push(i as u64 * block);
                from = end;
            }
        }
        Some(sizes) => {
            let mut at = 0;
            for size in sizes.iter().cycle() {
                if at >= n {
                    break;
                }
                let len = (*size).min(n - at);
                let mut enc = Encoder::new(EncoderConfig { level: 8, ..config }).unwrap();
                assert!(len <= enc.block_size(), "blocks up to {}", enc.block_size());
                let mut fixed = Vec::new();
                enc.encode_block(&samples[at * ch..(at + len) * ch], &mut fixed)
                    .unwrap();
                let h = parse_header(&fixed).unwrap();
                let mut frame = Vec::new();
                write_header(
                    &crossfade_flac::frame::FrameHeader {
                        variable_block_size: true,
                        number: at as u64,
                        ..h
                    },
                    &mut frame,
                );
                frame.extend(&fixed[h.header_len..fixed.len() - 2]);
                let crc = crc16(&frame);
                frame.extend(crc.to_be_bytes());
                frames.push(frame);
                starts.push(at as u64);
                at += len;
            }
        }
    }
    let sizes: Vec<usize> = frames.iter().map(Vec::len).collect();
    let block_sizes: Vec<u64> = starts
        .iter()
        .zip(starts.iter().skip(1).chain([&(n as u64)]))
        .map(|(a, b)| b - a)
        .collect();
    let info = crossfade_flac::streaminfo::StreamInfoBlock {
        min_block_size: *block_sizes.iter().min().unwrap() as u16,
        max_block_size: *block_sizes.iter().max().unwrap() as u16,
        min_frame_size: *sizes.iter().min().unwrap() as u32,
        max_frame_size: *sizes.iter().max().unwrap() as u32,
        sample_rate: 44_100,
        channels,
        bits_per_sample: bits,
        total_samples: if total { n as u64 } else { 0 },
        md5: [0; 16],
    };
    let mut bytes = b"fLaC".to_vec();
    block_header(0, table == Table::None, 34, &mut bytes);
    bytes.extend(info.write());
    let mut offsets = Vec::new();
    let mut off = 0u64;
    for s in &sizes {
        offsets.push(off);
        off += *s as u64;
    }
    let points: Vec<(u64, u64, u16)> = match table {
        Table::None => Vec::new(),
        Table::Every(k) => {
            let mut p: Vec<_> = (0..frames.len())
                .step_by(k)
                .map(|i| (starts[i], offsets[i], block_sizes[i] as u16))
                .collect();
            p.push((u64::MAX, 0, 0));
            p
        }
        Table::Wrong => (0..frames.len())
            .step_by(3)
            .map(|i| (starts[i], offsets[i] + 7 + i as u64, 1))
            .collect(),
    };
    if table != Table::None {
        block_header(3, true, points.len() * 18, &mut bytes);
        for (s, o, k) in points {
            bytes.extend(s.to_be_bytes());
            bytes.extend(o.to_be_bytes());
            bytes.extend(k.to_be_bytes());
        }
    }
    for f in &frames {
        bytes.extend(f);
    }
    Flac {
        bytes,
        frames: starts.into_iter().zip(sizes).collect(),
        max_frame: *frames
            .iter()
            .map(Vec::len)
            .collect::<Vec<_>>()
            .iter()
            .max()
            .unwrap(),
    }
}

/// Seeks to `target` and checks the reader lands on the packet that holds it and then reads
/// exactly what a read from the start reads from that packet on. `all`: the packets from the
/// start.
pub fn check_seek(d: &mut dyn Demuxer, all: &[Packet], target: i64, what: &str) {
    let end = all.last().map_or(0, |p| p.pts + p.duration as i64);
    let got = d
        .seek(target)
        .unwrap_or_else(|e| panic!("{what}: seek {target}: {e}"));
    let at = all
        .iter()
        .position(|p| p.pts + p.duration as i64 > target.max(0));
    match at {
        Some(i) => {
            assert_eq!(got, all[i].pts, "{what}: seek {target}");
            let rest = drain(d).unwrap();
            assert!(
                rest.len() == all.len() - i,
                "{what}: seek {target}: {} packets, want {}",
                rest.len(),
                all.len() - i
            );
            assert!(rest == all[i..], "{what}: seek {target}: packets differ");
        }
        None => {
            assert_eq!(got, end, "{what}: seek {target} past the end");
            assert!(!d.read_packet(&mut Packet::default()).unwrap(), "{what}");
        }
    }
}

/// 0, 1, the last frame, the end, past it, the same target twice, backwards after forwards and
/// a spread of others.
pub fn targets(end: i64, seed: u64) -> Vec<i64> {
    let mut rng = Lcg(seed);
    let mut t = vec![0, 1, end - 1, end, end + 1, -5];
    for _ in 0..12 {
        t.push(rng.below(end.max(1) as u64) as i64);
    }
    let again = t[8];
    t.extend([again, again, end / 2, end / 3, 2, i64::MAX]);
    t
}

pub fn check_all_targets(bytes: &[u8], what: &str) {
    let all = drain(open_bytes(bytes).as_mut()).unwrap();
    let end = all.last().map_or(0, |p| p.pts + p.duration as i64);
    let mut d = open_bytes(bytes);
    for t in targets(end, bytes.len() as u64) {
        check_seek(d.as_mut(), &all, t, what);
    }
    // A seek before anything was read, too.
    check_seek(open_bytes(bytes).as_mut(), &all, end / 2, what);
}
