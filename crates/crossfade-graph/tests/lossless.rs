//! Every lossless path through the engine, on random short signals and in-memory I/O: what goes
//! in comes out, and stream copy hands the muxer the demuxer's packets untouched.

use crossfade_core::{Limits, Packet, SampleFormat};
use crossfade_format::{FormatId, open_demuxer};
use crossfade_graph::{AudioSettings, CancelToken, Event, Job, JobInput, JobOutput, run};
use crossfade_testkit::io::FaultySource;
use crossfade_testkit::memory::MemoryIo;
use proptest::prelude::*;
use std::io::Cursor;

const ALL: [SampleFormat; 6] = [
    SampleFormat::U8,
    SampleFormat::S16,
    SampleFormat::S24,
    SampleFormat::S32,
    SampleFormat::F32,
    SampleFormat::F64,
];
/// What the FLAC encoder takes.
const FLAC_FORMATS: [SampleFormat; 3] = [SampleFormat::U8, SampleFormat::S16, SampleFormat::S24];
const RATES: [u32; 8] = [8000, 11_025, 22_050, 32_000, 44_100, 48_000, 88_200, 96_000];

#[derive(Debug, Clone, Copy)]
struct Case {
    ext: &'static str,
    format: SampleFormat,
    channels: u16,
    rate: u32,
    frames: usize,
    seed: u64,
    /// Bits shaved off integer samples, so some signals are quiet and compress.
    quiet: u32,
    level: Option<u8>,
}

fn case() -> impl Strategy<Value = Case> {
    let container = prop_oneof![
        prop::sample::select(ALL.to_vec()).prop_map(|f| ("wav", f)),
        prop::sample::select(ALL.to_vec()).prop_map(|f| ("aiff", f)),
        prop::sample::select(FLAC_FORMATS.to_vec()).prop_map(|f| ("flac", f)),
    ];
    (
        container,
        1u16..=8,
        prop::sample::select(RATES.to_vec()),
        prop_oneof![Just(0usize), 1..4500usize],
        any::<u64>(),
        0u32..24,
        prop::option::of(0u8..=8),
    )
        .prop_map(
            |((ext, format), channels, rate, frames, seed, quiet, level)| Case {
                ext,
                format,
                channels,
                rate,
                frames,
                seed,
                quiet,
                level: if ext == "flac" { level } else { None },
            },
        )
}

fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Interleaved little-endian samples of `format`, from nothing but the case's numbers.
fn pcm(c: &Case) -> Vec<u8> {
    let mut state = c.seed;
    let mut out = Vec::new();
    for _ in 0..c.frames * usize::from(c.channels) {
        let raw = splitmix(&mut state);
        match c.format {
            SampleFormat::F32 => {
                let x = (raw >> 11) as f64 / (1u64 << 52) as f64 - 1.0;
                out.extend_from_slice(&(x as f32).to_le_bytes());
            }
            SampleFormat::F64 => {
                let x = (raw >> 11) as f64 / (1u64 << 52) as f64 - 1.0;
                out.extend_from_slice(&x.to_le_bytes());
            }
            f => {
                let bits = f.bits() as u32;
                let v = ((raw as i64) >> (64 - bits)) >> c.quiet.min(bits - 1);
                if f == SampleFormat::U8 {
                    out.push((v + 128) as u8);
                } else {
                    out.extend_from_slice(&v.to_le_bytes()[..f.bytes_per_sample()]);
                }
            }
        }
    }
    out
}

/// A WAV file written here, not by the engine.
fn wav(c: &Case, data: &[u8]) -> Vec<u8> {
    let width = u16::try_from(c.format.bytes_per_sample()).unwrap();
    let bits = width * 8;
    let block = width * c.channels;
    let float = c.format.is_float();
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&(if float { 3u16 } else { 0xFFFE }).to_le_bytes());
    fmt.extend_from_slice(&c.channels.to_le_bytes());
    fmt.extend_from_slice(&c.rate.to_le_bytes());
    fmt.extend_from_slice(&(c.rate * u32::from(block)).to_le_bytes());
    fmt.extend_from_slice(&block.to_le_bytes());
    fmt.extend_from_slice(&bits.to_le_bytes());
    if !float {
        fmt.extend_from_slice(&22u16.to_le_bytes());
        fmt.extend_from_slice(&bits.to_le_bytes());
        fmt.extend_from_slice(&((1u32 << c.channels) - 1).to_le_bytes());
        fmt.extend_from_slice(&[
            1, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71,
        ]);
    }
    let mut body = b"WAVEfmt ".to_vec();
    body.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
    body.extend_from_slice(&fmt);
    body.extend_from_slice(b"data");
    body.extend_from_slice(&(data.len() as u32).to_le_bytes());
    body.extend_from_slice(data);
    if data.len() % 2 == 1 {
        body.push(0);
    }
    let mut v = b"RIFF".to_vec();
    v.extend_from_slice(&(body.len() as u32).to_le_bytes());
    v.extend_from_slice(&body);
    v
}

fn data_chunk(bytes: &[u8]) -> &[u8] {
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        if &bytes[pos..pos + 4] == b"data" {
            return &bytes[pos + 8..pos + 8 + len];
        }
        pos += 8 + len + (len & 1);
    }
    panic!("no data chunk");
}

fn convert(io: &mut MemoryIo, from: &str, to: &str, audio: AudioSettings) -> Vec<Event> {
    let job = Job {
        version: Job::VERSION,
        inputs: vec![JobInput { path: from.into() }],
        outputs: vec![JobOutput {
            path: to.into(),
            audio,
        }],
    };
    let mut events = Vec::new();
    run(
        &job,
        io,
        &Limits::default(),
        &CancelToken::new(),
        &mut |e| events.push(e.clone()),
    )
    .unwrap_or_else(|e| panic!("{from} -> {to}: {e}"));
    events
}

fn packets(bytes: Vec<u8>) -> (FormatId, Vec<(Vec<u8>, i64, u64)>) {
    let source = FaultySource::new(Cursor::new(bytes));
    let (format, mut demuxer) = open_demuxer(Box::new(source), &Limits::default()).unwrap();
    let mut packet = Packet::default();
    let mut all = Vec::new();
    while demuxer.read_packet(&mut packet).unwrap() {
        all.push((packet.data.clone(), packet.pts, packet.duration));
    }
    (format, all)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn encode_then_decode_returns_the_same_samples(c in case()) {
        let data = pcm(&c);
        let mut io = MemoryIo::new();
        io.insert("in.wav", wav(&c, &data));
        let encoded = format!("mid.{}", c.ext);
        let audio = AudioSettings { compression_level: c.level, ..AudioSettings::default() };
        convert(&mut io, "in.wav", &encoded, audio);
        convert(&mut io, &encoded, "back.wav", AudioSettings::default());
        let back = io.get("back.wav").unwrap();
        prop_assert!(data_chunk(&back) == data.as_slice(), "{c:?}");
    }

    #[test]
    fn stream_copy_returns_the_same_packets(c in case()) {
        let data = pcm(&c);
        let mut io = MemoryIo::new();
        io.insert("in.wav", wav(&c, &data));
        let name = format!("a.{}", c.ext);
        let copy = format!("b.{}", c.ext);
        let audio = AudioSettings { compression_level: c.level, ..AudioSettings::default() };
        convert(&mut io, "in.wav", &name, audio);
        let events = convert(
            &mut io,
            &name,
            &copy,
            AudioSettings { codec: Some("copy".into()), ..AudioSettings::default() },
        );
        let Some(Event::Started { plan }) = events.first() else { panic!("no start") };
        prop_assert!(plan.copy);
        let (original, copied) = (io.get(&name).unwrap(), io.get(&copy).unwrap());
        let (format_a, a) = packets(original.clone());
        let (format_b, b) = packets(copied.clone());
        prop_assert_eq!(format_a, format_b);
        prop_assert!(a == b, "{c:?}: packets differ");
        // Nothing but the audio is in these files, so the files themselves agree too.
        prop_assert!(original == copied, "{c:?}: files differ");
    }
}
