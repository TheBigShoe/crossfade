//! `crossfade convert` writing ALAC into M4A, end to end through the binary.

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

struct Dir(PathBuf);

impl Dir {
    fn new(test: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("crossfade-alac-{test}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
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

/// A 16-bit stereo WAV of a second: two tones.
fn wav() -> Vec<u8> {
    let frames = 44_100u32;
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + frames * 4).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&44_100u32.to_le_bytes());
    v.extend_from_slice(&(44_100u32 * 4).to_le_bytes());
    v.extend_from_slice(&4u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(frames * 4).to_le_bytes());
    for i in 0..frames {
        let t = f64::from(i) / 44_100.0;
        let l = (9000.0 * (std::f64::consts::TAU * 440.0 * t).sin()) as i16;
        let r = (7000.0 * (std::f64::consts::TAU * 660.0 * t).sin()) as i16;
        v.extend_from_slice(&l.to_le_bytes());
        v.extend_from_slice(&r.to_le_bytes());
    }
    v
}

#[test]
fn a_wav_goes_to_m4a_and_back_byte_for_byte() {
    let d = Dir::new("roundtrip");
    std::fs::write(d.path("in.wav"), wav()).unwrap();
    for args in [vec![], vec!["--codec", "alac"]] {
        let mut all = vec![
            "convert".to_string(),
            d.path("in.wav"),
            "-o".into(),
            d.path("out.m4a"),
            "--force".into(),
        ];
        all.extend(args.iter().map(|a| a.to_string()));
        let all: Vec<&str> = all.iter().map(String::as_str).collect();
        let out = run(&all);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    }
    let m4a = std::fs::metadata(d.path("out.m4a")).unwrap().len();
    assert!(m4a < 44_100 * 4 * 6 / 10, "two tones compress: {m4a} bytes");
    let report = text(&run(&["probe", &d.path("out.m4a")]).stdout);
    assert!(
        report.contains("alac") && report.contains("44100"),
        "{report}"
    );
    let out = run(&["convert", &d.path("out.m4a"), "-o", &d.path("back.wav")]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(
        std::fs::read(d.path("back.wav")).unwrap() == wav(),
        "lossless, header and all"
    );
    // Untouched into another M4A, and refused where it cannot go.
    let out = run(&[
        "convert",
        &d.path("out.m4a"),
        "-o",
        &d.path("copy.m4a"),
        "--codec",
        "copy",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let out = run(&[
        "convert",
        &d.path("in.wav"),
        "-o",
        &d.path("no.m4a"),
        "--sample-format",
        "f32",
    ]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("alac cannot store floating-point samples"),
        "{}",
        text(&out.stderr)
    );
    assert!(!std::path::Path::new(&d.path("no.m4a")).exists());
}
