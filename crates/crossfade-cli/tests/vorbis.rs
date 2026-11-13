//! Ogg Vorbis through the real binary: probe, and conversion to 16-bit with the lossy note
//! and to float with the decoder's own samples.

use crossfade_vorbis::testing::{self as w, OggLayout};
use crossfade_vorbis::{Decoder, Ident, Setup};
use std::path::PathBuf;
use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_crossfade"))
        .args(args)
        .output()
        .unwrap()
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// A unique scratch directory, removed on drop.
struct Dir(PathBuf);

impl Dir {
    fn new(test: &str) -> Self {
        let p =
            std::env::temp_dir().join(format!("crossfade-vorbis-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }

    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const SEQ: &str = "SSLLSLSSSLLLS";

/// The file, and its audible frames (priming 300 and padding 200 cut).
fn ogg() -> (Vec<u8>, w::SampleStream, u64) {
    let s = w::sample_stream(2, SEQ, 3);
    let layout = OggLayout {
        start: -300,
        end_trim: 200,
        ..OggLayout::default()
    };
    let bytes = w::ogg_file(
        [&s.headers[0], &s.headers[1], &s.headers[2]],
        &s.audio,
        &layout,
    );
    let total: i64 = w::yields(&s.headers[0], &s.headers[2], &s.audio)
        .iter()
        .sum();
    (bytes, s, (total - 300 - 200) as u64)
}

fn data_chunk(wav: &[u8]) -> &[u8] {
    let at = wav.windows(4).position(|w| w == b"data").unwrap();
    let len = u32::from_le_bytes(wav[at + 4..at + 8].try_into().unwrap()) as usize;
    &wav[at + 8..at + 8 + len]
}

#[test]
fn probe_shows_an_ogg_vorbis_file() {
    let d = Dir::new("probe");
    let (bytes, _, frames) = ogg();
    std::fs::write(d.path("in.ogg"), &bytes).unwrap();
    let out = run(&["probe", &d.path("in.ogg")]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let shown = text(&out.stdout);
    assert!(shown.starts_with("Input: ogg, "), "{shown}");
    assert!(
        shown.contains(&format!("audio, vorbis, 32000 Hz, stereo, f32, ")),
        "{shown}"
    );
    assert!(shown.contains(&format!("({frames} frames)")), "{shown}");
    assert!(shown.contains("TITLE"), "{shown}");
}

#[test]
fn vorbis_converts_to_16_bit_with_a_note_and_to_float_exactly() {
    let d = Dir::new("convert");
    let (bytes, s, frames) = ogg();
    std::fs::write(d.path("in.ogg"), &bytes).unwrap();

    let out = run(&["convert", &d.path("in.ogg"), "-o", &d.path("o16.wav")]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let said = text(&out.stdout) + &text(&out.stderr);
    assert!(
        said.contains("vorbis has no bit depth: writing 16-bit; choose --sample-format to change"),
        "{said}"
    );
    let wav = std::fs::read(d.path("o16.wav")).unwrap();
    assert_eq!(data_chunk(&wav).len() as u64, frames * 2 * 2);

    let out = run(&[
        "convert",
        &d.path("in.ogg"),
        "-o",
        &d.path("of.wav"),
        "--sample-format",
        "f32",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    // The decoder's own output, priming dropped and padding cut.
    let ident = Ident::parse(&s.headers[0]).unwrap();
    let setup = Setup::parse(&s.headers[2], &ident, usize::MAX).unwrap();
    let mut dec = Decoder::new(&ident, setup).unwrap();
    let mut pcm = vec![];
    for p in &s.audio {
        dec.decode(p, &mut pcm).unwrap();
    }
    let want: Vec<u8> = pcm[300 * 2..(300 + frames as usize) * 2]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let wav = std::fs::read(d.path("of.wav")).unwrap();
    assert!(data_chunk(&wav) == want, "float output differs");
}
