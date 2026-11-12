//! The milestone's exit criterion: seek-then-decode equals decode-then-slice, exactly, through
//! `DecodeSession`, for WAV, AIFF, FLAC (with and without a SEEKTABLE, fixed and variable block
//! size, 16 and 24 bit, mono, stereo and 5.1) and Ogg FLAC, at one thread and several.

use crossfade_codec::{EncodeOptions, encoder_for};
use crossfade_core::{
    AudioFrame, AudioSpec, ChannelLayout, CodecId, Error, Limits, MediaSink, Packet, SampleFormat,
    Samples, StreamInfo, Tags, TimeBase,
};
use crossfade_format::{FormatId, create_muxer};
use crossfade_graph::DecodeSession;
use crossfade_testkit::io::FaultySource;
use crossfade_testkit::memory::MemoryIo;
use proptest::prelude::*;
use std::io::{Cursor, SeekFrom};
use std::sync::{Arc, LazyLock, Mutex};

const RATE: u32 = 44_100;

#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Cursor<Vec<u8>>>>);

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

impl Shared {
    fn bytes(&self) -> Vec<u8> {
        self.0.lock().unwrap().get_ref().clone()
    }
}

/// Noise over a ramp, as container samples of `format`.
fn signal(frames: usize, channels: usize, format: SampleFormat, seed: u64) -> Vec<i32> {
    let bits = format.bits() as u32;
    let mut x = seed;
    (0..frames * channels)
        .map(|i| {
            x = x
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let amp = 1i64 << (bits - 2);
            let ramp = ((i / channels) as i64 * 31) % (2 * amp) - amp;
            let noise = (x >> 40) as i64 % (1 << (bits - 4));
            (ramp / 2 + noise) as i32
        })
        .collect()
}

fn frame_of(spec: &AudioSpec, samples: &[i32], pts: i64) -> AudioFrame {
    let mut f = AudioFrame::new(spec);
    f.pts = pts;
    f.samples = match spec.format() {
        SampleFormat::S16 => Samples::S16(samples.iter().map(|&s| s as i16).collect()),
        SampleFormat::S24 => Samples::S24(samples.to_vec()),
        other => panic!("{other:?}"),
    };
    f
}

/// Encodes `samples` in `chunk`-frame frames and writes them with `format`'s muxer.
/// `per_packet`: one encoder per packet of that many frames (variable block size FLAC).
fn write(
    format: FormatId,
    codec: CodecId,
    spec: &AudioSpec,
    samples: &[i32],
    options: EncodeOptions,
) -> Vec<u8> {
    let ch = usize::from(spec.layout().channels());
    let mut enc = encoder_for(codec, spec, &options).unwrap();
    let mut packets = Vec::new();
    let mut p = Packet::default();
    for (i, chunk) in samples.chunks(1000 * ch).enumerate() {
        enc.send_frame(&frame_of(spec, chunk, (i * 1000) as i64))
            .unwrap();
        while enc.receive_packet(&mut p).unwrap() {
            packets.push(p.clone());
        }
    }
    enc.drain();
    while enc.receive_packet(&mut p).unwrap() {
        packets.push(p.clone());
    }
    let stream = StreamInfo {
        extradata: enc.extradata().to_vec(),
        frames: Some((samples.len() / ch) as u64),
        ..StreamInfo::new(
            0,
            codec,
            spec.clone(),
            TimeBase::new(1, i64::from(RATE)).unwrap(),
        )
    };
    mux(format, &stream, &packets, enc.extradata())
}

fn mux(format: FormatId, stream: &StreamInfo, packets: &[Packet], extradata: &[u8]) -> Vec<u8> {
    let sink = Shared::default();
    let mut m = create_muxer(format, Box::new(sink.clone()), stream, &Tags::new()).unwrap();
    for p in packets {
        m.write_packet(p).unwrap();
    }
    if !extradata.is_empty() {
        m.set_extradata(extradata).unwrap();
    }
    m.finish().unwrap();
    sink.bytes()
}

fn crc8(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |mut c, &b| {
        c ^= b;
        for _ in 0..8 {
            c = if c & 0x80 != 0 {
                (c << 1) ^ 0x07
            } else {
                c << 1
            };
        }
        c
    })
}

fn crc16(data: &[u8]) -> u16 {
    data.iter().fold(0u16, |mut c, &b| {
        c ^= u16::from(b) << 8;
        for _ in 0..8 {
            c = if c & 0x8000 != 0 {
                (c << 1) ^ 0x8005
            } else {
                c << 1
            };
        }
        c
    })
}

/// FLAC's UTF-8-like coded number.
fn coded(n: u64) -> Vec<u8> {
    if n < 0x80 {
        return vec![n as u8];
    }
    let len = (2..=7).find(|&l| n < 1u64 << (5 * l + 1)).unwrap();
    let mut out = vec![(0xFF00u16 >> len) as u8 | (n >> (6 * (len - 1))) as u8];
    for i in (0..len - 1).rev() {
        out.push(0x80 | ((n >> (6 * i)) & 0x3F) as u8);
    }
    out
}

/// The same frame with the variable blocking strategy, numbered by its first sample.
fn to_variable(frame: &[u8], first_sample: u64) -> Vec<u8> {
    let number_len = match frame[4].leading_ones() {
        0 => 1,
        n => n as usize,
    };
    let bs_extra = match frame[2] >> 4 {
        6 => 1,
        7 => 2,
        _ => 0,
    };
    let sr_extra = match frame[2] & 15 {
        12 => 1,
        13 | 14 => 2,
        _ => 0,
    };
    let extras = 4 + number_len..4 + number_len + bs_extra + sr_extra;
    let header_len = extras.end + 1;
    let mut out = vec![0xFF, 0xF9, frame[2], frame[3]];
    out.extend(coded(first_sample));
    out.extend(&frame[extras]);
    out.push(crc8(&out));
    out.extend(&frame[header_len..frame.len() - 2]);
    let crc = crc16(&out);
    out.extend(crc.to_be_bytes());
    out
}

fn streaminfo(spec: &AudioSpec, bits: u8, blocks: (u16, u16), total: u64) -> Vec<u8> {
    let mut b = blocks.0.to_be_bytes().to_vec();
    b.extend(blocks.1.to_be_bytes());
    b.extend([0; 6]);
    let ch = u64::from(spec.layout().channels() - 1);
    let packed = u64::from(spec.sample_rate()) << 44 | ch << 41 | u64::from(bits - 1) << 36 | total;
    b.extend(packed.to_be_bytes());
    b.extend([0; 16]);
    b
}

/// A variable block size FLAC stream: each block of `sizes` (cycled) coded by its own encoder.
fn variable(format: FormatId, spec: &AudioSpec, samples: &[i32], sizes: &[usize]) -> Vec<u8> {
    let ch = usize::from(spec.layout().channels());
    let n = samples.len() / ch;
    let mut packets = Vec::new();
    let mut at = 0;
    for &size in sizes.iter().cycle() {
        if at >= n {
            break;
        }
        let len = size.min(n - at);
        let mut enc = encoder_for(CodecId::Flac, spec, &EncodeOptions::default()).unwrap();
        enc.send_frame(&frame_of(spec, &samples[at * ch..(at + len) * ch], 0))
            .unwrap();
        enc.drain();
        let mut p = Packet::default();
        assert!(enc.receive_packet(&mut p).unwrap());
        assert!(
            !enc.receive_packet(&mut p.clone()).unwrap(),
            "one frame per block"
        );
        packets.push(Packet {
            data: to_variable(&p.data, at as u64),
            pts: at as i64,
            duration: len as u64,
            ..Packet::default()
        });
        at += len;
    }
    let bits = spec.format().bits() as u8;
    let lo = *sizes.iter().min().unwrap() as u16;
    let hi = *sizes.iter().max().unwrap() as u16;
    let info = streaminfo(spec, bits, (lo, hi), n as u64);
    let stream = StreamInfo {
        extradata: info.clone(),
        frames: Some(n as u64),
        ..StreamInfo::new(
            0,
            CodecId::Flac,
            spec.clone(),
            TimeBase::new(1, i64::from(RATE)).unwrap(),
        )
    };
    mux(format, &stream, &packets, &info)
}

/// The file with a SEEKTABLE after its other metadata: a point every `every` frames.
fn with_seektable(flac: &[u8], every: usize) -> Vec<u8> {
    let (_, mut d) =
        crossfade_format::open_demuxer(Box::new(Cursor::new(flac.to_vec())), &Limits::default())
            .unwrap();
    let mut packets = Vec::new();
    let mut p = Packet::default();
    while d.read_packet(&mut p).unwrap() {
        packets.push(p.clone());
    }
    let mut points = Vec::new();
    let mut offset = 0u64;
    for (i, p) in packets.into_iter().enumerate() {
        if i % every == 0 {
            points.extend((p.pts as u64).to_be_bytes());
            points.extend(offset.to_be_bytes());
            points.extend((p.duration as u16).to_be_bytes());
        }
        offset += p.data.len() as u64;
    }
    let mut at = 4;
    let mut out = b"fLaC".to_vec();
    loop {
        let last = flac[at] & 0x80 != 0;
        let len = u32::from_be_bytes([0, flac[at + 1], flac[at + 2], flac[at + 3]]) as usize;
        out.push(flac[at] & 0x7F);
        out.extend(&flac[at + 1..at + 4 + len]);
        at += 4 + len;
        if last {
            break;
        }
    }
    out.push(0x83);
    out.extend(&(points.len() as u32).to_be_bytes()[1..]);
    out.extend(points);
    out.extend(&flac[at..]);
    out
}

/// A reader of `bytes` through the engine's own I/O seam.
fn open(bytes: Vec<u8>, threads: usize) -> Session {
    let mut io = MemoryIo::new();
    io.insert("in", bytes);
    let limits = Limits {
        max_threads: threads,
        ..Limits::default()
    };
    Session(DecodeSession::open("in", &mut io, &limits).unwrap())
}

struct Session(DecodeSession);

impl Session {
    /// Every sample from here on, flattened, checking the pts run on without a gap from the
    /// session's position.
    fn rest(&mut self) -> Vec<i64> {
        let mut out = Vec::new();
        let mut f = AudioFrame::new(&self.0.stream().audio);
        let mut pts = self.0.position();
        while self.0.next_frame(&mut f).unwrap() {
            assert_eq!(f.pts as u64, pts, "frames follow on");
            assert!(f.frames() > 0, "no empty frames");
            pts += f.frames() as u64;
            assert_eq!(self.0.position(), pts);
            out.extend(flat(&f.samples));
        }
        out
    }
}

fn flat(s: &Samples) -> Vec<i64> {
    match s {
        Samples::U8(v) => v.iter().map(|&x| i64::from(x)).collect(),
        Samples::S16(v) => v.iter().map(|&x| i64::from(x)).collect(),
        Samples::S24(v) | Samples::S32(v) => v.iter().map(|&x| i64::from(x)).collect(),
        Samples::F32(v) => v.iter().map(|&x| i64::from(x.to_bits())).collect(),
        Samples::F64(v) => v.iter().map(|&x| x.to_bits() as i64).collect(),
    }
}

/// Seek-then-decode equals decode-then-slice at every target, at `threads`.
fn check(bytes: &[u8], targets: &[u64], threads: usize, what: &str) {
    let mut whole = open(bytes.to_vec(), threads);
    let ch = whole.0.stream().audio.layout().channels() as usize;
    let all = whole.rest();
    let total = (all.len() / ch) as u64;
    if let Some(frames) = whole.0.stream().frames {
        assert_eq!(frames, total, "{what}");
    }
    let mut s = open(bytes.to_vec(), threads);
    for &t in targets {
        s.0.seek(t)
            .unwrap_or_else(|e| panic!("{what}: seek {t}: {e}"));
        let at = t.min(total);
        if s.0.stream().frames.is_some() || t <= total {
            assert_eq!(s.0.position(), at, "{what}: seek {t}");
        }
        let got = s.rest();
        assert_eq!(s.0.position(), total, "{what}: seek {t}, at the end");
        assert!(
            got == all[at as usize * ch..],
            "{what}: seek {t}: {} samples, want {}",
            got.len(),
            all.len() - at as usize * ch
        );
    }
}

/// 0, 1, the last frame, the end, past it, the same target twice, backwards after forwards.
fn edge_targets(total: u64) -> Vec<u64> {
    vec![
        0,
        1,
        total - 1,
        total,
        total + 1000,
        total / 2,
        total / 2,
        total / 3,
        4095,
        4096,
        4097,
        u64::MAX,
        7,
        0,
    ]
}

const FRAMES: usize = 40_013;

struct Case {
    name: String,
    bytes: Vec<u8>,
}

fn layouts() -> [(u16, ChannelLayout); 3] {
    [
        (1, ChannelLayout::mono()),
        (2, ChannelLayout::stereo()),
        (6, ChannelLayout::from_name("5.1").unwrap()),
    ]
}

static CASES: LazyLock<Vec<Case>> = LazyLock::new(|| {
    let mut cases = Vec::new();
    for (seed, (channels, layout)) in layouts().into_iter().enumerate() {
        for format in [SampleFormat::S16, SampleFormat::S24] {
            let spec = AudioSpec::new(RATE, format, layout.clone()).unwrap();
            let samples = signal(FRAMES, usize::from(channels), format, seed as u64 + 1);
            let tag = format!("{channels}ch {format:?}");
            let (wav, aiff) = match format {
                SampleFormat::S16 => (CodecId::PcmS16Le, CodecId::PcmS16Be),
                _ => (CodecId::PcmS24Le, CodecId::PcmS24Be),
            };
            let opts = EncodeOptions::default();
            cases.push(Case {
                name: format!("wav {tag}"),
                bytes: write(FormatId::Wav, wav, &spec, &samples, opts),
            });
            cases.push(Case {
                name: format!("aiff {tag}"),
                bytes: write(FormatId::Aiff, aiff, &spec, &samples, opts),
            });
            let fixed = write(FormatId::Flac, CodecId::Flac, &spec, &samples, opts);
            cases.push(Case {
                name: format!("flac {tag} with a seektable"),
                bytes: with_seektable(&fixed, 4),
            });
            cases.push(Case {
                name: format!("flac {tag}"),
                bytes: fixed,
            });
            let sizes = [4096, 37, 1152, 4000, 576];
            let var = variable(FormatId::Flac, &spec, &samples, &sizes);
            cases.push(Case {
                name: format!("variable flac {tag} with a seektable"),
                bytes: with_seektable(&var, 3),
            });
            cases.push(Case {
                name: format!("variable flac {tag}"),
                bytes: var,
            });
            cases.push(Case {
                name: format!("ogg flac {tag}"),
                bytes: write(FormatId::Ogg, CodecId::Flac, &spec, &samples, opts),
            });
            cases.push(Case {
                name: format!("variable ogg flac {tag}"),
                bytes: variable(FormatId::Ogg, &spec, &samples, &sizes),
            });
        }
    }
    cases
});

#[test]
fn every_case_decodes_to_the_signal_it_was_made_from() {
    for (seed, (channels, _)) in layouts().into_iter().enumerate() {
        for format in [SampleFormat::S16, SampleFormat::S24] {
            let want: Vec<i64> = signal(FRAMES, usize::from(channels), format, seed as u64 + 1)
                .into_iter()
                .map(i64::from)
                .collect();
            let tag = format!("{channels}ch {format:?}");
            for case in CASES
                .iter()
                .filter(|c| c.name.ends_with(&tag) || c.name.contains(&format!("{tag} ")))
            {
                assert!(open(case.bytes.clone(), 1).rest() == want, "{}", case.name);
            }
        }
    }
}

#[test]
fn seek_then_decode_equals_decode_then_slice_at_the_edges() {
    for case in CASES.iter() {
        for threads in [1, 4] {
            check(
                &case.bytes,
                &edge_targets(FRAMES as u64),
                threads,
                &format!("{} at {threads} threads", case.name),
            );
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]
    #[test]
    fn seek_then_decode_equals_decode_then_slice(
        targets in proptest::collection::vec(0u64..FRAMES as u64 + 200, 1..6),
        threads in prop_oneof![Just(1usize), Just(3), Just(0)],
        which in 0usize..1000,
    ) {
        let case = &CASES[which % CASES.len()];
        check(&case.bytes, &targets, threads, &case.name);
    }
}

/// Every committed reader fixture that decodes, at its edges and a spread of targets.
#[test]
fn the_committed_fixtures_seek_exactly() {
    let root = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../crossfade-format/tests/fixtures"
    );
    let mut paths = Vec::new();
    for dir in ["flac", "ogg", "aiff", "wav"] {
        for e in std::fs::read_dir(format!("{root}/{dir}")).unwrap() {
            let p = e.unwrap().path();
            if p.extension().is_some_and(|x| {
                ["flac", "oga", "aiff", "aifc", "wav"]
                    .iter()
                    .any(|k| x == *k)
            }) {
                paths.push(p);
            }
        }
    }
    paths.sort();
    let mut seen = 0;
    for p in paths {
        let bytes = std::fs::read(&p).unwrap();
        let mut io = MemoryIo::new();
        io.insert("in", bytes.clone());
        // Fixtures that do not decode whole (a fuzzer's regression input) are not this test's
        // business.
        let Ok(mut s) = DecodeSession::open("in", &mut io, &Limits::default()) else {
            continue;
        };
        let mut f = AudioFrame::new(&s.stream().audio);
        let whole = loop {
            match s.next_frame(&mut f) {
                Ok(true) => {}
                Ok(false) => break true,
                Err(_) => break false,
            }
        };
        if !whole {
            continue;
        }
        let total = s.stream().frames.unwrap_or(1000).max(2);
        let mut targets = edge_targets(total);
        targets.extend((1..9).map(|k| total * k / 9 + k));
        for threads in [1, 3] {
            check(&bytes, &targets, threads, &p.display().to_string());
        }
        seen += 1;
    }
    assert!(seen >= 40, "{seen} fixtures");
}

#[test]
fn a_session_on_a_pipe_decodes_but_cannot_seek() {
    let case = &CASES[0];
    let mut io = PipeIo(case.bytes.clone());
    let mut s = DecodeSession::open("in", &mut io, &Limits::default()).unwrap();
    assert!(matches!(s.seek(10), Err(Error::Unsupported { .. })));
    let mut f = AudioFrame::new(&s.stream().audio);
    assert!(s.next_frame(&mut f).unwrap());
    assert_eq!(f.pts, 0);
}

struct PipeIo(Vec<u8>);

impl crossfade_core::JobIo for PipeIo {
    fn open_source(&mut self, _: &str) -> Result<Box<dyn crossfade_core::MediaSource>, Error> {
        Ok(Box::new(
            FaultySource::new(Cursor::new(self.0.clone())).not_seekable(),
        ))
    }
    fn create_sink(&mut self, _: &str) -> Result<Box<dyn MediaSink>, Error> {
        unreachable!()
    }
    fn commit(&mut self, _: &str) -> Result<(), Error> {
        unreachable!()
    }
    fn discard(&mut self, _: &str) {}
}
