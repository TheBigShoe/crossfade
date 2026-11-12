//! AAC through the real binary: decoded with `--features aac` (the first lossy decoder, so the
//! lossy-output rule is checked end to end here), refused by name without it.

use std::path::PathBuf;
use std::process::{Command, Output};

const TONE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../crossfade-format/tests/fixtures/adts/tone_44k_stereo.aac"
);

/// A scratch directory, removed on drop.
struct Dir(PathBuf);

impl Dir {
    fn new(test: &str) -> Self {
        let p = std::env::temp_dir().join(format!("crossfade-aac-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
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

    fn exists(&self, name: &str) -> bool {
        self.0.join(name).exists()
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

#[cfg(not(feature = "aac"))]
#[test]
fn without_the_feature_converting_aac_is_refused_and_says_why() {
    let dir = Dir::new("refused");
    let out = dir.run(&["convert", TONE, "-o", "out.wav"]);
    assert_eq!(out.status.code(), Some(1));
    let err = text(&out.stderr);
    assert!(
        err.contains(
            "aac decoding is not part of this build (AAC is patent-encumbered); build with --features aac"
        ),
        "{err}"
    );
    assert!(!dir.exists("out.wav"));
}

#[cfg(feature = "aac")]
mod with_the_feature {
    use super::*;

    /// (format tag, channels, rate, bits) and the data chunk of a WAV file.
    fn wav(bytes: &[u8]) -> ((u16, u16, u32, u16), Vec<u8>) {
        let mut at = 12;
        let (mut fmt, mut data) = ((0, 0, 0, 0), Vec::new());
        while at + 8 <= bytes.len() {
            let id = &bytes[at..at + 4];
            let n = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
            let body = &bytes[at + 8..at + 8 + n];
            let u16_at = |i: usize| u16::from_le_bytes([body[i], body[i + 1]]);
            if id == b"fmt " {
                let mut tag = u16_at(0);
                if tag == 0xFFFE {
                    tag = u16_at(24);
                }
                let rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                fmt = (tag, u16_at(2), rate, u16_at(14));
            }
            if id == b"data" {
                data = body.to_vec();
            }
            at += 8 + n + (n & 1);
        }
        (fmt, data)
    }

    fn s16(data: &[u8]) -> Vec<i16> {
        data.chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect()
    }

    #[test]
    fn converting_aac_writes_dithered_16_bit_with_the_lossy_note() {
        let dir = Dir::new("convert");
        let ok = |args: &[&str]| {
            let out = dir.run(args);
            assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
            text(&out.stderr)
        };
        let note = "crossfade: note: aac has no bit depth: writing 16-bit; choose --sample-format \
                    to change\n";
        assert_eq!(ok(&["convert", TONE, "-o", "tpdf.wav"]), note);
        assert_eq!(
            ok(&["convert", TONE, "-o", "none.wav", "--dither", "none"]),
            note
        );
        assert_eq!(
            ok(&["convert", TONE, "-o", "f32.wav", "--sample-format", "f32"]),
            ""
        );
        let (fmt, tpdf) = wav(&dir.read("tpdf.wav"));
        assert_eq!(fmt, (1, 2, 44100, 16));
        let (fmt, none) = wav(&dir.read("none.wav"));
        assert_eq!(fmt, (1, 2, 44100, 16));
        let (fmt, float) = wav(&dir.read("f32.wav"));
        assert_eq!(fmt, (3, 2, 44100, 32));
        // ADTS has no priming information: every decoded frame is written.
        assert_eq!(float.len() / 8, 46 * 1024);
        let float: Vec<f32> = float
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        let (tpdf, none) = (s16(&tpdf), s16(&none));
        assert_eq!((tpdf.len(), none.len()), (float.len(), float.len()));
        // Without dither, each sample is the float rounded; with it, within two steps of
        // that and not the same.
        for (&n, &f) in none.iter().zip(&float) {
            assert!(
                (f64::from(n) - f64::from(f) * 32768.0).abs() <= 1.0,
                "{n} {f}"
            );
        }
        assert_ne!(tpdf, none);
        assert!(
            tpdf.iter()
                .zip(&none)
                .all(|(&t, &n)| (i32::from(t) - i32::from(n)).abs() <= 2)
        );
        // A -6 dBFS tone: the peak is near half scale.
        let peak = float.iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!((0.4..0.6).contains(&peak), "{peak}");
    }

    #[test]
    fn probe_names_the_stream() {
        let dir = Dir::new("probe");
        let out = dir.run(&["probe", TONE]);
        assert_eq!(out.status.code(), Some(0));
        let shown = text(&out.stdout);
        assert!(
            shown.contains("Stream 0: audio, aac, 44100 Hz, stereo, f32, 1.068 s (47104 frames)"),
            "{shown}"
        );
    }
}
