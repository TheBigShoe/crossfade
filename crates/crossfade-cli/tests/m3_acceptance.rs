//! Milestone 3 acceptance: FLAC, AIFF and Ogg FLAC through the real binary. Inputs come from
//! the testkit's signals or the committed fixtures; every result is judged by the parsers and
//! arithmetic in this file, not by the engine's own readers.
//!
//! The FLAC fixtures' signal generator is shared by path with the format crate's tests
//! (`#[path]` below), so the samples that were encoded are regenerated, not copied.

#[path = "../../crossfade-format/tests/support/signals.rs"]
mod signals;

use crossfade_core::{AudioSpec, ChannelLayout, SampleFormat};
use crossfade_testkit::{pcm, signal};
use std::path::PathBuf;
use std::process::{Command, Output};

const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../crossfade-format/tests/fixtures"
);

fn fixture(relative: &str) -> Vec<u8> {
    let path = format!("{FIXTURES}/{relative}");
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// A unique scratch directory, removed on drop.
struct Dir(PathBuf);

impl Dir {
    fn new(test: &str) -> Self {
        let p = std::env::temp_dir().join(format!("crossfade-m3-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn write(&self, name: &str, bytes: &[u8]) {
        std::fs::write(self.path(name), bytes).unwrap();
    }

    fn read(&self, name: &str) -> Vec<u8> {
        std::fs::read(self.path(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    // Relative names and a fixed cwd keep messages and listings stable.
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_crossfade"))
            .args(args)
            .current_dir(&self.0)
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{args:?}: {}",
            text(&out.stderr)
        );
        assert!(out.stderr.is_empty(), "{args:?}: {}", text(&out.stderr));
        text(&out.stdout)
    }

    fn listing(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// The command must exit 1 with one `crossfade: ...` line naming `cause`, and leave the
    /// directory exactly as it was: no output, no temporary file.
    fn refused(&self, args: &[&str], cause: &str) -> String {
        let before = self.listing();
        let out = self.run(args);
        let err = text(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{args:?}: {err}");
        assert!(out.stdout.is_empty());
        assert!(err.starts_with("crossfade: "), "{err}");
        assert_eq!(err.lines().count(), 1, "one line expected: {err}");
        assert!(err.contains(cause), "{cause:?} missing from {err}");
        assert_eq!(self.listing(), before, "{args:?} left files behind");
        err.trim_end().to_owned()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

// ---- WAV, by hand --------------------------------------------------------------------------

fn bits(format: SampleFormat) -> u16 {
    u16::try_from(format.bits()).unwrap()
}

/// A WAV file: plain `fmt` where the old format allows it, WAVE_FORMAT_EXTENSIBLE for more than
/// two channels or more than 16 bits.
fn wav(rate: u32, channels: u16, format: SampleFormat, data: &[u8]) -> Vec<u8> {
    let width = u16::try_from(format.bytes_per_sample()).unwrap();
    let block = width * channels;
    let float = format.is_float();
    let extensible = !float && (channels > 2 || bits(format) > 16);
    let mut fmt = Vec::new();
    let tag: u16 = if extensible {
        0xFFFE
    } else if float {
        3
    } else {
        1
    };
    fmt.extend_from_slice(&tag.to_le_bytes());
    fmt.extend_from_slice(&channels.to_le_bytes());
    fmt.extend_from_slice(&rate.to_le_bytes());
    fmt.extend_from_slice(&(rate * u32::from(block)).to_le_bytes());
    fmt.extend_from_slice(&block.to_le_bytes());
    fmt.extend_from_slice(&bits(format).to_le_bytes());
    if extensible {
        fmt.extend_from_slice(&22u16.to_le_bytes());
        fmt.extend_from_slice(&bits(format).to_le_bytes());
        let mask: u32 = match channels {
            1 => 0x4,
            2 => 0x3,
            n => (1 << n) - 1,
        };
        fmt.extend_from_slice(&mask.to_le_bytes());
        fmt.extend_from_slice(&[
            1, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71,
        ]);
    }
    let mut body = b"WAVEfmt ".to_vec();
    body.extend_from_slice(&u32::try_from(fmt.len()).unwrap().to_le_bytes());
    body.extend_from_slice(&fmt);
    body.extend_from_slice(b"data");
    body.extend_from_slice(&u32::try_from(data.len()).unwrap().to_le_bytes());
    body.extend_from_slice(data);
    if data.len() % 2 == 1 {
        body.push(0);
    }
    let mut v = b"RIFF".to_vec();
    v.extend_from_slice(&u32::try_from(body.len()).unwrap().to_le_bytes());
    v.extend_from_slice(&body);
    v
}

/// Walks the RIFF chunks; `(id, body)` of each.
fn riff_chunks(bytes: &[u8]) -> Vec<([u8; 4], &[u8])> {
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    let mut out = Vec::new();
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id: [u8; 4] = bytes[pos..pos + 4].try_into().unwrap();
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = pos + 8;
        out.push((id, &bytes[body..body + len]));
        // Chunks are padded to even length.
        pos = body + len + (len & 1);
    }
    out
}

fn data_chunk(bytes: &[u8]) -> &[u8] {
    riff_chunks(bytes)
        .into_iter()
        .find(|(id, _)| id == b"data")
        .expect("no data chunk")
        .1
}

/// `(channels, rate, bits)` from the `fmt ` chunk.
fn wav_format(bytes: &[u8]) -> (u16, u32, u16) {
    let fmt = riff_chunks(bytes)
        .into_iter()
        .find(|(id, _)| id == b"fmt ")
        .expect("no fmt chunk")
        .1;
    (
        u16::from_le_bytes([fmt[2], fmt[3]]),
        u32::from_le_bytes(fmt[4..8].try_into().unwrap()),
        u16::from_le_bytes([fmt[14], fmt[15]]),
    )
}

const INFO_IDS: [(&[u8; 4], &str); 8] = [
    (b"INAM", "TITLE"),
    (b"IART", "ARTIST"),
    (b"IPRD", "ALBUM"),
    (b"ICRD", "DATE"),
    (b"ICMT", "COMMENT"),
    (b"IGNR", "GENRE"),
    (b"ITRK", "TRACKNUMBER"),
    (b"ICOP", "COPYRIGHT"),
];

type Tags = Vec<(String, String)>;

fn wav_tags(bytes: &[u8]) -> Tags {
    let mut out = Tags::new();
    for (id, body) in riff_chunks(bytes) {
        if &id != b"LIST" || &body[..4] != b"INFO" {
            continue;
        }
        let mut pos = 4;
        while pos + 8 <= body.len() {
            let sub: [u8; 4] = body[pos..pos + 4].try_into().unwrap();
            let len = u32::from_le_bytes(body[pos + 4..pos + 8].try_into().unwrap()) as usize;
            let value = &body[pos + 8..pos + 8 + len];
            let value = text(value.strip_suffix(&[0]).unwrap_or(value));
            let key = INFO_IDS
                .iter()
                .find(|(i, _)| **i == sub)
                .expect("known id")
                .1;
            out.push((key.to_owned(), value));
            pos += 8 + len + (len & 1);
        }
    }
    out
}

/// Deterministic tonal audio, a different sine on every channel: interleaved little-endian
/// bytes in `format`.
fn tonal(rate: u32, channels: u16, format: SampleFormat, frames: usize) -> Vec<u8> {
    let mono = AudioSpec::new(rate, format, ChannelLayout::mono()).unwrap();
    let per_channel: Vec<Vec<f64>> = (0..channels)
        .map(|c| {
            let sine = signal::Signal::Sine {
                hz: 220.0 * f64::from(c + 1),
                amplitude: 0.5,
            };
            signal::render(&sine, &mono, frames).unwrap()
        })
        .collect();
    let mut interleaved = Vec::new();
    for f in 0..frames {
        for ch in &per_channel {
            interleaved.push(ch[f]);
        }
    }
    pcm::encode_le(&interleaved, format)
}

const FORMATS: [SampleFormat; 6] = [
    SampleFormat::U8,
    SampleFormat::S16,
    SampleFormat::S24,
    SampleFormat::S32,
    SampleFormat::F32,
    SampleFormat::F64,
];

// ---- AIFF, by hand -------------------------------------------------------------------------

struct Aiff {
    form: [u8; 4],
    channels: u16,
    frames: u32,
    bits: u16,
    rate: f64,
    compression: Option<[u8; 4]>,
    sound: Vec<u8>,
    /// `NAME`, `AUTH`, `(c) ` and `ANNO` chunks as tag keys, in file order.
    tags: Tags,
}

fn parse_aiff(bytes: &[u8]) -> Aiff {
    assert_eq!(&bytes[..4], b"FORM");
    let form: [u8; 4] = bytes[8..12].try_into().unwrap();
    assert!(&form == b"AIFF" || &form == b"AIFC", "{form:?}");
    let (mut channels, mut frames, mut bits, mut rate, mut compression) = (0, 0, 0, 0.0, None);
    let (mut sound, mut tags) = (Vec::new(), Tags::new());
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id: [u8; 4] = bytes[pos..pos + 4].try_into().unwrap();
        let len = u32::from_be_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = &bytes[pos + 8..pos + 8 + len];
        match &id {
            b"COMM" => {
                channels = u16::from_be_bytes([body[0], body[1]]);
                frames = u32::from_be_bytes(body[2..6].try_into().unwrap());
                bits = u16::from_be_bytes([body[6], body[7]]);
                // 80-bit extended: sign and 15-bit exponent, then a 64-bit mantissa.
                let exp = i32::from(u16::from_be_bytes([body[8], body[9]]) & 0x7FFF) - 16383;
                let mantissa = u64::from_be_bytes(body[10..18].try_into().unwrap());
                rate = mantissa as f64 * 2f64.powi(exp - 63);
                if &form == b"AIFC" {
                    compression = Some(body[18..22].try_into().unwrap());
                }
            }
            b"SSND" => {
                let offset = u32::from_be_bytes(body[..4].try_into().unwrap()) as usize;
                assert_eq!(offset, 0, "the tests only expect aligned sound data");
                sound = body[8..].to_vec();
            }
            b"NAME" => tags.push(("TITLE".into(), text(body))),
            b"AUTH" => tags.push(("ARTIST".into(), text(body))),
            b"(c) " => tags.push(("COPYRIGHT".into(), text(body))),
            b"ANNO" => tags.push(("COMMENT".into(), text(body))),
            _ => {}
        }
        pos += 8 + len + (len & 1);
    }
    Aiff {
        form,
        channels,
        frames,
        bits,
        rate,
        compression,
        sound,
        tags,
    }
}

/// What AIFF stores for little-endian WAV sample bytes: big-endian words, and signed 8-bit
/// where WAV has offset-binary.
fn aiff_sound_from_wav(le: &[u8], format: SampleFormat) -> Vec<u8> {
    if format == SampleFormat::U8 {
        return le.iter().map(|b| b ^ 0x80).collect();
    }
    le.chunks_exact(format.bytes_per_sample())
        .flat_map(|s| s.iter().rev().copied())
        .collect()
}

// ---- FLAC, by hand -------------------------------------------------------------------------

struct Flac {
    /// The 34-byte STREAMINFO body.
    streaminfo: Vec<u8>,
    tags: Tags,
    /// Everything after the last metadata block: the frames.
    audio: Vec<u8>,
}

fn parse_flac(bytes: &[u8]) -> Flac {
    assert_eq!(&bytes[..4], b"fLaC");
    let mut pos = 4;
    let (mut streaminfo, mut tags) = (Vec::new(), Tags::new());
    loop {
        let head = bytes[pos];
        let len = u32::from_be_bytes([0, bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]]) as usize;
        let body = bytes[pos + 4..pos + 4 + len].to_vec();
        pos += 4 + len;
        match head & 0x7F {
            0 => streaminfo = body,
            4 => tags = parse_vorbis_comment(&body),
            _ => {}
        }
        if head & 0x80 != 0 {
            break;
        }
    }
    Flac {
        streaminfo,
        tags,
        audio: bytes[pos..].to_vec(),
    }
}

fn parse_vorbis_comment(body: &[u8]) -> Tags {
    let le = |at: usize| u32::from_le_bytes(body[at..at + 4].try_into().unwrap()) as usize;
    let vendor = le(0);
    let count = le(4 + vendor);
    let mut pos = 8 + vendor;
    let mut out = Tags::new();
    for _ in 0..count {
        let len = le(pos);
        let entry = text(&body[pos + 4..pos + 4 + len]);
        let (k, v) = entry.split_once('=').expect("KEY=value");
        out.push((k.to_owned(), v.to_owned()));
        pos += 4 + len;
    }
    out
}

fn vorbis_comment(tags: &[(&str, &str)]) -> Vec<u8> {
    let vendor = b"crossfade-test";
    let mut out = (vendor.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(vendor);
    out.extend_from_slice(&(tags.len() as u32).to_le_bytes());
    for (k, v) in tags {
        let entry = format!("{k}={v}");
        out.extend_from_slice(&(entry.len() as u32).to_le_bytes());
        out.extend_from_slice(entry.as_bytes());
    }
    out
}

/// `base`'s STREAMINFO and frames under exactly these tags, with no other metadata block.
fn flac_with_tags(base: &[u8], tags: &[(&str, &str)]) -> Vec<u8> {
    let parsed = parse_flac(base);
    let comment = vorbis_comment(tags);
    let mut out = b"fLaC".to_vec();
    out.push(0);
    out.extend_from_slice(&[0, 0, 34]);
    out.extend_from_slice(&parsed.streaminfo);
    out.push(0x80 | 4);
    out.extend_from_slice(&(comment.len() as u32).to_be_bytes()[1..]);
    out.extend_from_slice(&comment);
    out.extend_from_slice(&parsed.audio);
    out
}

/// CRC-16 of a FLAC frame: polynomial 0x8005, initial value 0, no reflection.
fn crc16(data: &[u8]) -> u16 {
    data.iter().fold(0u16, |mut crc, &b| {
        crc ^= u16::from(b) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x8005
            } else {
                crc << 1
            };
        }
        crc
    })
}

/// Changes the audio of the first frame and fixes its CRC-16, so the file still splits into
/// frames but no longer decodes to what its STREAMINFO MD5 describes.
fn flac_with_damaged_audio(original: &[u8]) -> Vec<u8> {
    let parsed = parse_flac(original);
    let start = original.len() - parsed.audio.len();
    // A frame ends where the CRC-16 over it comes to zero and the next frame's sync follows.
    let end = (start + 16..original.len() - 2)
        .find(|&e| crc16(&original[start..e]) == 0 && original[e..e + 2] == [0xFF, 0xF8])
        .expect("a second frame follows the first");
    let mut bytes = original.to_vec();
    for b in &mut bytes[start + 24..start + 32] {
        *b ^= 0x5A;
    }
    let crc = crc16(&bytes[start..end - 2]);
    bytes[end - 2..end].copy_from_slice(&crc.to_be_bytes());
    assert_ne!(bytes, original);
    bytes
}

// ---- probes --------------------------------------------------------------------------------

fn probe_fixture(relative: &str, json: bool) -> String {
    let d = Dir::new(&format!("probe-{}", relative.replace(['/', '.'], "-")));
    d.write("input", &fixture(relative));
    let mut args = vec!["probe", "input"];
    if json {
        args.push("--json");
    }
    d.ok(&args)
}

#[test]
fn probe_tagged_flac() {
    insta::assert_snapshot!(probe_fixture("flac/tagged.flac", false), @r"
    Input: flac, 12691 bytes
      Stream 0: audio, flac, 44100 Hz, stereo, s16, 0.113 s (5000 frames)
    Tags:
      TITLE: Fixture
      ARTIST: Crossfade
    ");
}

#[test]
fn probe_json_of_the_20_bit_flac() {
    insta::assert_snapshot!(probe_fixture("flac/stereo-20bit.flac", true), @r#"
    {
      "schema": 1,
      "format": "flac",
      "size_bytes": 9757,
      "streams": [
        {
          "index": 0,
          "type": "audio",
          "codec": "flac",
          "sample_format": "s24",
          "bits_per_raw_sample": 20,
          "sample_rate": 48000,
          "channels": 2,
          "layout": "stereo",
          "frames": 4000,
          "duration_seconds": 0.08333333333333333
        }
      ],
      "tags": []
    }
    "#);
}

#[test]
fn probe_aiff_c_float() {
    insta::assert_snapshot!(probe_fixture("aiff/f32.aifc", false), @r"
    Input: aiff, 8896 bytes
      Stream 0: audio, pcm_f32be, 22050 Hz, stereo, f32, 0.027 s (600 frames)
    ");
}

#[test]
fn probe_json_of_the_tagged_ogg_flac() {
    insta::assert_snapshot!(probe_fixture("ogg/tagged.oga", true), @r#"
    {
      "schema": 1,
      "format": "ogg",
      "size_bytes": 11950,
      "streams": [
        {
          "index": 0,
          "type": "audio",
          "codec": "flac",
          "sample_format": "s16",
          "bits_per_raw_sample": null,
          "sample_rate": 44100,
          "channels": 2,
          "layout": "stereo",
          "frames": 4410,
          "duration_seconds": 0.1
        }
      ],
      "tags": [
        {
          "key": "TITLE",
          "value": "Crossfade"
        },
        {
          "key": "ARTIST",
          "value": "Tester"
        },
        {
          "key": "ALBUM",
          "value": "Fixtures"
        }
      ]
    }
    "#);
}

// ---- WAV <-> FLAC --------------------------------------------------------------------------

#[test]
fn wav_to_flac_and_back_is_bit_exact_for_u8_s16_s24_in_mono_stereo_and_5_1() {
    let d = Dir::new("wav-flac-wav");
    for format in [SampleFormat::U8, SampleFormat::S16, SampleFormat::S24] {
        for channels in [1u16, 2, 6] {
            let data = tonal(48_000, channels, format, 6000);
            d.write("in.wav", &wav(48_000, channels, format, &data));
            d.ok(&["convert", "in.wav", "-o", "mid.flac", "--force"]);
            d.ok(&["convert", "mid.flac", "-o", "out.wav", "--force"]);
            let out = d.read("out.wav");
            let case = format!("{format:?} x {channels}");
            assert_eq!(wav_format(&out), (channels, 48_000, bits(format)), "{case}");
            assert!(data_chunk(&out) == data, "{case}: samples differ");
            // The FLAC file itself is what STREAMINFO says it is: 20 bits of rate, 3 of
            // channels minus one, 5 of depth minus one.
            let info = parse_flac(&d.read("mid.flac")).streaminfo;
            let packed = u64::from_be_bytes(info[10..18].try_into().unwrap());
            assert_eq!(packed >> 44, 48_000, "{case}");
            assert_eq!(((packed >> 41) & 7) + 1, u64::from(channels), "{case}");
            assert_eq!(((packed >> 36) & 31) + 1, u64::from(bits(format)), "{case}");
            assert_eq!(packed & 0xF_FFFF_FFFF, 6000, "{case}");
        }
    }
}

#[test]
fn every_compression_level_decodes_to_the_same_audio_and_level_8_is_not_larger_than_0() {
    let d = Dir::new("levels");
    for (format, channels) in [(SampleFormat::S16, 2u16), (SampleFormat::S24, 6)] {
        let data = tonal(48_000, channels, format, 12_000);
        d.write("in.wav", &wav(48_000, channels, format, &data));
        let mut sizes = Vec::new();
        for level in 0..=8 {
            let level = level.to_string();
            d.ok(&[
                "convert",
                "in.wav",
                "-o",
                "mid.flac",
                "--compression-level",
                &level,
                "--force",
            ]);
            sizes.push(d.read("mid.flac").len());
            d.ok(&["convert", "mid.flac", "-o", "out.wav", "--force"]);
            assert!(
                data_chunk(&d.read("out.wav")) == data,
                "{format:?} x {channels}, level {level}"
            );
        }
        assert!(sizes[8] <= sizes[0], "{format:?} x {channels}: {sizes:?}");
    }
}

#[test]
fn an_explicit_flac_codec_is_the_default_for_a_flac_output() {
    let d = Dir::new("codec-flac");
    d.write(
        "in.wav",
        &wav(
            48_000,
            2,
            SampleFormat::S16,
            &tonal(48_000, 2, SampleFormat::S16, 3000),
        ),
    );
    d.ok(&["convert", "in.wav", "-o", "a.flac"]);
    d.ok(&["convert", "in.wav", "-o", "b.flac", "--codec", "flac"]);
    assert_eq!(d.read("a.flac"), d.read("b.flac"));
}

// ---- WAV <-> AIFF --------------------------------------------------------------------------

#[test]
fn wav_and_aiff_are_bit_exact_both_ways_for_all_six_sample_formats() {
    let d = Dir::new("wav-aiff");
    for format in FORMATS {
        let data = tonal(44_100, 2, format, 3000);
        d.write("in.wav", &wav(44_100, 2, format, &data));
        d.ok(&["convert", "in.wav", "-o", "mid.aiff", "--force"]);
        let aiff = parse_aiff(&d.read("mid.aiff"));
        let case = format!("{format:?}");
        let float = format.is_float();
        // AIFF proper has no float samples: those go out as AIFF-C.
        assert_eq!(&aiff.form, if float { b"AIFC" } else { b"AIFF" }, "{case}");
        let name: Option<&[u8; 4]> = match format {
            SampleFormat::F32 => Some(b"fl32"),
            SampleFormat::F64 => Some(b"fl64"),
            _ => None,
        };
        assert_eq!(aiff.compression.as_ref(), name, "{case}");
        assert_eq!(aiff.channels, 2, "{case}");
        assert_eq!(aiff.frames, 3000, "{case}");
        assert_eq!(aiff.bits, bits(format), "{case}");
        assert_eq!(aiff.rate, 44_100.0, "{case}");
        assert!(
            aiff.sound == aiff_sound_from_wav(&data, format),
            "{case}: sound data"
        );

        d.ok(&["convert", "mid.aiff", "-o", "out.wav", "--force"]);
        let out = d.read("out.wav");
        assert_eq!(wav_format(&out), (2, 44_100, bits(format)), "{case}");
        assert!(data_chunk(&out) == data, "{case}: back to WAV");
    }
}

#[test]
fn aiff_to_flac_and_back_keeps_the_integer_formats_flac_can_hold() {
    let d = Dir::new("aiff-flac-aiff");
    for format in [SampleFormat::U8, SampleFormat::S16, SampleFormat::S24] {
        let data = tonal(44_100, 2, format, 3000);
        d.write("in.wav", &wav(44_100, 2, format, &data));
        d.ok(&["convert", "in.wav", "-o", "in.aiff", "--force"]);
        let source = parse_aiff(&d.read("in.aiff"));
        d.ok(&["convert", "in.aiff", "-o", "mid.flac", "--force"]);
        d.ok(&["convert", "mid.flac", "-o", "out.aiff", "--force"]);
        let out = parse_aiff(&d.read("out.aiff"));
        let case = format!("{format:?}");
        assert!(out.sound == source.sound, "{case}");
        assert!(out.sound == aiff_sound_from_wav(&data, format), "{case}");
        assert_eq!(
            (out.channels, out.frames, out.bits),
            (2, 3000, bits(format)),
            "{case}"
        );
        assert_eq!(out.rate, 44_100.0, "{case}");
    }
}

// ---- FLAC fixtures -------------------------------------------------------------------------

/// The WAV data a fixture's signal should come out as: the decoder's output format is the
/// smallest of u8/s16/s24/s32 that holds the depth, with the samples left-justified in it.
fn expected_wav_data(signal: &signals::Signal) -> (SampleFormat, Vec<u8>) {
    let format = match signal.bits {
        1..=8 => SampleFormat::U8,
        9..=16 => SampleFormat::S16,
        17..=24 => SampleFormat::S24,
        _ => SampleFormat::S32,
    };
    let shift = format.bits() as u32 - u32::from(signal.bits);
    let mut out = Vec::new();
    for &s in &signal.samples {
        let v = i64::from(s) << shift;
        if format == SampleFormat::U8 {
            out.push((v + 128) as u8);
        } else {
            out.extend_from_slice(&v.to_le_bytes()[..format.bytes_per_sample()]);
        }
    }
    (format, out)
}

#[test]
fn every_flac_fixture_converts_to_a_wav_equal_to_its_regenerated_signal() {
    let d = Dir::new("fixtures");
    for &(file, name) in signals::FIXTURES {
        let want = signals::signal(name);
        let (format, data) = expected_wav_data(&want);
        d.write("in.flac", &fixture(&format!("flac/{file}")));
        d.ok(&["convert", "in.flac", "-o", "out.wav", "--force"]);
        let out = d.read("out.wav");
        let channels = u16::try_from(want.channels).unwrap();
        assert_eq!(
            wav_format(&out),
            (channels, want.rate, bits(format)),
            "{file}"
        );
        assert!(data_chunk(&out) == data, "{file}: samples differ");
    }
}

#[test]
fn the_20_bit_flac_converts_to_24_bit_samples_shifted_left_by_four() {
    let d = Dir::new("twenty-bit");
    d.write("in.flac", &fixture("flac/stereo-20bit.flac"));
    let probe = d.ok(&["probe", "in.flac", "--json"]);
    let json: serde_json::Value = serde_json::from_str(&probe).unwrap();
    assert_eq!(json["streams"][0]["sample_format"], "s24");
    assert_eq!(json["streams"][0]["bits_per_raw_sample"], 20);

    d.ok(&["convert", "in.flac", "-o", "out.wav"]);
    let out = d.read("out.wav");
    assert_eq!(wav_format(&out), (2, 48_000, 24));
    let source = signals::signal("stereo20");
    assert_eq!(source.bits, 20);
    let mut want = Vec::new();
    for s in &source.samples {
        want.extend_from_slice(&(s << 4).to_le_bytes()[..3]);
    }
    assert!(data_chunk(&out) == want);
    // The WAV has no notion of a 20-bit depth.
    let again: serde_json::Value =
        serde_json::from_str(&d.ok(&["probe", "out.wav", "--json"])).unwrap();
    assert_eq!(
        again["streams"][0]["bits_per_raw_sample"],
        serde_json::Value::Null
    );
}

// ---- stream copy ---------------------------------------------------------------------------

#[test]
fn flac_to_flac_copy_keeps_streaminfo_and_every_frame_byte_identical() {
    let d = Dir::new("copy-flac");
    for file in [
        "stereo-16bit.flac",
        "surround-5.1.flac",
        "block-16.flac",
        "stereo-24bit.flac",
    ] {
        let source = fixture(&format!("flac/{file}"));
        d.write("in.flac", &source);
        d.ok(&[
            "convert", "in.flac", "-o", "out.flac", "--codec", "copy", "--force",
        ]);
        let (a, b) = (parse_flac(&source), parse_flac(&d.read("out.flac")));
        assert_eq!(a.streaminfo, b.streaminfo, "{file}");
        assert!(a.audio == b.audio, "{file}: frames differ");
    }
}

#[test]
fn ogg_flac_copy_writes_the_frames_of_its_native_twin_and_the_same_audio() {
    let d = Dir::new("copy-ogg");
    for name in ["stereo16", "mono24", "tagged", "smallblock", "noise8"] {
        d.write("in.oga", &fixture(&format!("ogg/{name}.oga")));
        let twin = fixture(&format!("ogg/{name}.flac"));
        d.ok(&[
            "convert", "in.oga", "-o", "out.flac", "--codec", "copy", "--force",
        ]);
        let (want, got) = (parse_flac(&twin), parse_flac(&d.read("out.flac")));
        assert!(
            got.audio == want.audio,
            "{name}: frames differ from the native twin"
        );
        // MD5 and total samples describe the audio, so they match whatever the block sizes.
        assert_eq!(
            got.streaminfo[18..],
            want.streaminfo[18..],
            "{name}: md5, total samples"
        );
        assert_eq!(
            got.streaminfo[10..18],
            want.streaminfo[10..18],
            "{name}: format"
        );

        d.write("twin.flac", &twin);
        d.ok(&["convert", "out.flac", "-o", "a.wav", "--force"]);
        d.ok(&["convert", "twin.flac", "-o", "b.wav", "--force"]);
        d.ok(&["convert", "in.oga", "-o", "c.wav", "--force"]);
        let (a, b, c) = (d.read("a.wav"), d.read("b.wav"), d.read("c.wav"));
        assert!(a == b && b == c, "{name}: decoded audio differs");
    }
}

#[test]
fn copy_never_needs_a_decoder() {
    let d = Dir::new("copy-damaged");
    let source = fixture("flac/stereo-16bit.flac");
    let damaged = flac_with_damaged_audio(&source);
    d.write("damaged.flac", &damaged);

    // The frames still split and pass their CRCs, but the audio is no longer what STREAMINFO's
    // MD5 was taken over.
    let err = d.refused(&["convert", "damaged.flac", "-o", "out.wav"], "MD5");
    assert!(err.contains("does not match the MD5"), "{err}");

    d.ok(&[
        "convert",
        "damaged.flac",
        "-o",
        "out.flac",
        "--codec",
        "copy",
    ]);
    let (a, b) = (parse_flac(&damaged), parse_flac(&d.read("out.flac")));
    assert!(a.audio == b.audio);
    assert_eq!(a.streaminfo, b.streaminfo);
}

// ---- tags ----------------------------------------------------------------------------------

fn pairs(list: &[(&str, &str)]) -> Tags {
    list.iter()
        .map(|&(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

/// Every key the writers know, some twice, and one no container but FLAC has a place for.
const RICH: [(&str, &str); 11] = [
    ("TITLE", "Song"),
    ("ARTIST", "One"),
    ("ARTIST", "Two"),
    ("ALBUM", "Disc"),
    ("DATE", "2024"),
    ("TRACKNUMBER", "7"),
    ("GENRE", "Test"),
    ("COMMENT", "first"),
    ("COMMENT", "second"),
    ("COPYRIGHT", "(c) Me"),
    ("CROSSFADE_TEST", "custom"),
];

#[test]
fn tagged_flac_keeps_every_tag_in_order_when_re_encoded_and_when_copied() {
    let d = Dir::new("tags-flac");
    d.write(
        "in.flac",
        &flac_with_tags(&fixture("flac/stereo-16bit.flac"), &RICH),
    );
    for extra in [&[][..], &["--codec", "flac"], &["--codec", "copy"]] {
        let mut args = vec!["convert", "in.flac", "-o", "out.flac", "--force"];
        args.extend_from_slice(extra);
        d.ok(&args);
        assert_eq!(
            parse_flac(&d.read("out.flac")).tags,
            pairs(&RICH),
            "{extra:?}"
        );
    }

    // The fixture with a seek table, a picture and padding keeps the tags it has.
    d.write("fx.flac", &fixture("flac/tagged.flac"));
    d.ok(&["convert", "fx.flac", "-o", "fx-out.flac"]);
    let want = pairs(&[("TITLE", "Fixture"), ("ARTIST", "Crossfade")]);
    assert_eq!(parse_flac(&d.read("fx-out.flac")).tags, want);
}

#[test]
fn wav_holds_eight_keys_and_the_first_value_of_each() {
    let d = Dir::new("tags-wav");
    d.write(
        "in.flac",
        &flac_with_tags(&fixture("flac/stereo-16bit.flac"), &RICH),
    );
    d.ok(&["convert", "in.flac", "-o", "mid.wav"]);
    // INFO order: title, artist, album, date, comment, genre, track, copyright.
    let kept = pairs(&[
        ("TITLE", "Song"),
        ("ARTIST", "One"),
        ("ALBUM", "Disc"),
        ("DATE", "2024"),
        ("COMMENT", "first"),
        ("GENRE", "Test"),
        ("TRACKNUMBER", "7"),
        ("COPYRIGHT", "(c) Me"),
    ]);
    assert_eq!(wav_tags(&d.read("mid.wav")), kept);
    d.ok(&["convert", "mid.wav", "-o", "out.flac"]);
    assert_eq!(parse_flac(&d.read("out.flac")).tags, kept);
}

#[test]
fn aiff_holds_title_artist_copyright_and_every_comment() {
    let d = Dir::new("tags-aiff");
    d.write(
        "in.flac",
        &flac_with_tags(&fixture("flac/stereo-16bit.flac"), &RICH),
    );
    d.ok(&["convert", "in.flac", "-o", "mid.aiff"]);
    let kept = pairs(&[
        ("TITLE", "Song"),
        ("ARTIST", "One"),
        ("COPYRIGHT", "(c) Me"),
        ("COMMENT", "first"),
        ("COMMENT", "second"),
    ]);
    assert_eq!(parse_aiff(&d.read("mid.aiff")).tags, kept);
    d.ok(&["convert", "mid.aiff", "-o", "out.flac"]);
    assert_eq!(parse_flac(&d.read("out.flac")).tags, kept);
}

#[test]
fn ogg_flac_tags_reach_the_flac_output() {
    let d = Dir::new("tags-ogg");
    d.write("in.oga", &fixture("ogg/tagged.oga"));
    let want = pairs(&[
        ("TITLE", "Crossfade"),
        ("ARTIST", "Tester"),
        ("ALBUM", "Fixtures"),
    ]);
    for extra in [&[][..], &["--codec", "copy"]] {
        let mut args = vec!["convert", "in.oga", "-o", "out.flac", "--force"];
        args.extend_from_slice(extra);
        d.ok(&args);
        assert_eq!(parse_flac(&d.read("out.flac")).tags, want, "{extra:?}");
    }
}

// ---- refusals ------------------------------------------------------------------------------

/// CRC-32 of an Ogg page: polynomial 0x04C11DB7, initial value 0, no reflection.
fn ogg_crc(data: &[u8]) -> u32 {
    data.iter().fold(0u32, |mut crc, &b| {
        crc ^= u32::from(b) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
        crc
    })
}

/// A one-page Ogg stream whose first packet is a Vorbis identification header.
fn ogg_vorbis() -> Vec<u8> {
    let mut packet = b"\x01vorbis".to_vec();
    packet.extend_from_slice(&0u32.to_le_bytes()); // version
    packet.push(2); // channels
    packet.extend_from_slice(&44_100u32.to_le_bytes());
    packet.extend_from_slice(&[0; 12]);
    packet.extend_from_slice(&[0xB8, 1]);
    let mut page = b"OggS\0\x02".to_vec();
    page.extend_from_slice(&0i64.to_le_bytes());
    page.extend_from_slice(&7u32.to_le_bytes()); // serial
    page.extend_from_slice(&0u32.to_le_bytes()); // sequence
    page.extend_from_slice(&0u32.to_le_bytes()); // checksum, patched below
    page.push(1);
    page.push(u8::try_from(packet.len()).unwrap());
    page.extend_from_slice(&packet);
    let crc = ogg_crc(&page);
    page[22..26].copy_from_slice(&crc.to_le_bytes());
    page
}

fn tonal_wav(rate: u32, channels: u16, format: SampleFormat) -> Vec<u8> {
    wav(rate, channels, format, &tonal(rate, channels, format, 2000))
}

#[test]
fn unwritable_combinations_are_refused_with_one_line_and_no_files() {
    let d = Dir::new("refusals");
    d.write("s16.wav", &tonal_wav(48_000, 2, SampleFormat::S16));
    d.write("f32.wav", &tonal_wav(48_000, 2, SampleFormat::F32));
    d.write("s32.wav", &tonal_wav(48_000, 2, SampleFormat::S32));
    d.write("hi.wav", &tonal_wav(192_000, 2, SampleFormat::S16));
    d.write("in.flac", &fixture("flac/stereo-16bit.flac"));

    let cases: [(&[&str], &str); 11] = [
        (
            &["convert", "f32.wav", "-o", "o.flac"],
            "flac cannot store floating-point samples",
        ),
        (
            &["convert", "s32.wav", "-o", "o.flac"],
            "flac encoding of 32-bit samples is not supported yet",
        ),
        (
            &["convert", "hi.wav", "-o", "o.flac"],
            "flac encoding supports sample rates up to 96000 Hz, got 192000",
        ),
        (
            &["convert", "s16.wav", "-o", "o.aiff", "--codec", "copy"],
            "cannot copy pcm_s16le into aiff",
        ),
        (
            &["convert", "s16.wav", "-o", "o.wav", "--codec", "flac"],
            "codec flac cannot be written into wav",
        ),
        (
            &[
                "convert",
                "s16.wav",
                "-o",
                "o.wav",
                "--compression-level",
                "5",
            ],
            "--compression-level applies to flac only",
        ),
        (
            &[
                "convert",
                "s16.wav",
                "-o",
                "o.wav",
                "--codec",
                "copy",
                "--sample-format",
                "s24",
            ],
            "--codec copy cannot change the sample format",
        ),
        (
            &["convert", "s16.wav", "-o", "o.ogg"],
            ".ogg can be read but not written yet",
        ),
        (
            &["convert", "in.flac", "-o", "o.oga"],
            ".ogg can be read but not written yet",
        ),
        (
            &["convert", "s16.wav", "-o", "o.mp3"],
            ".wav, .flac, .aiff/.aif/.aifc",
        ),
        (
            &[
                "convert",
                "in.flac",
                "-o",
                "o.flac",
                "--codec",
                "copy",
                "--compression-level",
                "3",
            ],
            "--codec copy cannot take a compression level",
        ),
    ];
    for (args, cause) in cases {
        d.refused(args, cause);
    }
}

#[test]
fn an_ogg_vorbis_input_is_identified_and_refused() {
    let d = Dir::new("vorbis");
    d.write("in.ogg", &ogg_vorbis());
    d.refused(
        &["convert", "in.ogg", "-o", "out.wav"],
        "ogg stream holds vorbis",
    );
    d.refused(&["probe", "in.ogg"], "ogg stream holds vorbis");
}

#[test]
fn a_truncated_flac_says_where_it_ended_and_leaves_nothing() {
    let d = Dir::new("truncated-flac");
    let source = fixture("flac/stereo-16bit.flac");
    // Past the metadata, half way through the frames.
    let cut = source.len() * 3 / 5;
    d.write("cut.flac", &source[..cut]);
    for out in ["out.wav", "out.flac", "out.aiff"] {
        let err = d.refused(
            &["convert", "cut.flac", "-o", out],
            "unexpected end of input",
        );
        assert_eq!(
            err,
            format!("crossfade: unexpected end of input at byte {cut}")
        );
    }
}

#[test]
fn a_flac_whose_audio_fails_its_md5_is_refused_and_leaves_nothing() {
    let d = Dir::new("md5");
    let mut bytes = fixture("flac/stereo-16bit.flac");
    // STREAMINFO's body starts at byte 8; its MD5 is the last 16 of 34 bytes.
    bytes[8 + 18] ^= 0xFF;
    d.write("bad.flac", &bytes);
    for out in ["out.wav", "out.flac"] {
        d.refused(
            &["convert", "bad.flac", "-o", out],
            "does not match the MD5",
        );
    }
}

// ---- jobs ----------------------------------------------------------------------------------

#[test]
fn emit_job_shows_the_new_fields_only_when_they_are_set() {
    let d = Dir::new("emit-job");
    let plain = d.ok(&["convert", "a.wav", "-o", "b.flac", "--emit-job"]);
    assert!(
        !plain.contains("codec") && !plain.contains("compression_level"),
        "{plain}"
    );
    let level_only = d.ok(&[
        "convert",
        "a.wav",
        "-o",
        "b.flac",
        "--compression-level",
        "0",
        "--emit-job",
    ]);
    assert!(level_only.contains("\"compression_level\": 0") && !level_only.contains("codec"));

    let full = d.ok(&[
        "convert",
        "a.wav",
        "-o",
        "b.flac",
        "--sample-format",
        "s24",
        "--codec",
        "flac",
        "--compression-level",
        "5",
        "--emit-job",
    ]);
    insta::assert_snapshot!(full, @r#"
    {
      "version": 1,
      "inputs": [
        {
          "path": "a.wav"
        }
      ],
      "outputs": [
        {
          "path": "b.flac",
          "audio": {
            "sample_format": "s24",
            "codec": "flac",
            "compression_level": 5
          }
        }
      ]
    }
    "#);
    assert_eq!(
        d.listing(),
        Vec::<String>::new(),
        "--emit-job writes nothing"
    );
}

#[test]
fn a_job_file_from_milestone_1_still_loads_and_runs() {
    use crossfade_core::Limits;
    use crossfade_graph::{CancelToken, Job, run};

    let d = Dir::new("m1-job");
    let data = tonal(48_000, 2, SampleFormat::S16, 1000);
    d.write("sine.wav", &wav(48_000, 2, SampleFormat::S16, &data));
    // Exactly what milestone 1's `--emit-job` printed: no codec, no compression level.
    let json = r#"{
      "version": 1,
      "inputs": [ { "path": "IN" } ],
      "outputs": [ { "path": "OUT", "audio": { "sample_format": "s24" } } ]
    }"#
    .replace("IN", &d.path("sine.wav").to_string_lossy())
    .replace("OUT", &d.path("out.wav").to_string_lossy());
    let job = Job::from_json(&json).unwrap();
    assert_eq!(job.outputs[0].audio.codec, None);
    assert_eq!(job.outputs[0].audio.compression_level, None);

    let mut io = crossfade_app::FsIo::new();
    run(
        &job,
        &mut io,
        &Limits::default(),
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap();
    let out = d.read("out.wav");
    assert_eq!(wav_format(&out), (2, 48_000, 24));
    // s16 into s24 is a shift left by eight.
    let want: Vec<u8> = data.chunks(2).flat_map(|s| [0, s[0], s[1]]).collect();
    assert!(data_chunk(&out) == want);
}
