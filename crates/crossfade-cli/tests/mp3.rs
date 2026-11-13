//! MP3 through the command line: probe, and convert to WAV and FLAC (16-bit with the note about
//! lossy sources, dithered unless `--dither none`; `--sample-format f32` for float).

use crossfade_core::{Demuxer, Limits, Packet, SampleFormat};
use std::path::PathBuf;
use std::process::{Command, Output};

struct Dir(PathBuf);

impl Dir {
    fn new(test: &str) -> Self {
        let d = std::env::temp_dir().join(format!("crossfade-mp3-{test}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let fixture = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../crossfade-format/tests/fixtures/mp3/tagged.mp3"
        );
        std::fs::copy(fixture, d.join("in.mp3")).unwrap();
        Self(d)
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_crossfade"))
            .args(args)
            .current_dir(&self.0)
            .output()
            .unwrap()
    }

    fn read(&self, name: &str) -> Vec<u8> {
        std::fs::read(self.0.join(name)).unwrap()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8(b.to_vec()).unwrap()
}

/// What a written file holds: its container's reader and every packet's bytes.
fn read_back(bytes: Vec<u8>) -> (crossfade_format::FormatId, Box<dyn Demuxer>, Vec<u8>) {
    let (format, mut d) =
        crossfade_format::open_demuxer(Box::new(std::io::Cursor::new(bytes)), &Limits::default())
            .unwrap();
    let mut all = Vec::new();
    let mut p = Packet::default();
    while d.read_packet(&mut p).unwrap() {
        all.extend_from_slice(&p.data);
    }
    (format, d, all)
}

#[test]
fn probe_names_the_container_codec_rate_channels_duration_and_tags() {
    let dir = Dir::new("probe");
    let out = dir.run(&["probe", "in.mp3"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stdout),
        "Input: mp3, 9568 bytes\n  Stream 0: audio, mp3, 44100 Hz, stereo, f32, 0.500 s (22050 frames)\nTags:\n  TITLE: Title\n  ARTIST: Artist\n  ALBUM: Album\n  DATE: 2024\n  TRACKNUMBER: 3/9\n  GENRE: Rock\n  COMMENT: comment\n"
    );
}

#[test]
fn convert_to_wav_writes_16_bit_with_the_note_and_the_exact_length() {
    let dir = Dir::new("wav");
    let out = dir.run(&["convert", "in.mp3", "-o", "out.wav"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(
        text(&out.stderr)
            .contains("mp3 has no bit depth: writing 16-bit; choose --sample-format to change"),
        "{}",
        text(&out.stderr)
    );
    let (format, d, data) = read_back(dir.read("out.wav"));
    assert_eq!(format, crossfade_format::FormatId::Wav);
    let s = &d.streams()[0];
    assert_eq!(
        (s.audio.format(), s.audio.sample_rate()),
        (SampleFormat::S16, 44100)
    );
    assert_eq!(data.len(), 22050 * 2 * 2);
    assert_eq!(d.tags().get("TITLE"), Some("Title"));

    // Dither is on by default: without it the samples differ.
    let out = dir.run(&["convert", "in.mp3", "-o", "plain.wav", "--dither", "none"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let (_, _, plain) = read_back(dir.read("plain.wav"));
    assert_eq!(plain.len(), data.len());
    assert_ne!(plain, data);
}

#[test]
fn convert_to_float_wav_and_to_flac() {
    let dir = Dir::new("f32-flac");
    let out = dir.run(&[
        "convert",
        "in.mp3",
        "-o",
        "out.wav",
        "--sample-format",
        "f32",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(!text(&out.stderr).contains("no bit depth"));
    let (_, d, data) = read_back(dir.read("out.wav"));
    assert_eq!(d.streams()[0].audio.format(), SampleFormat::F32);
    assert_eq!(data.len(), 22050 * 2 * 4);

    let out = dir.run(&["convert", "in.mp3", "-o", "out.flac"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let (format, d, _) = read_back(dir.read("out.flac"));
    assert_eq!(format, crossfade_format::FormatId::Flac);
    let s = &d.streams()[0];
    assert_eq!(
        (s.audio.format(), s.frames),
        (SampleFormat::S16, Some(22050))
    );
    assert_eq!(d.tags().get("ARTIST"), Some("Artist"));
}
