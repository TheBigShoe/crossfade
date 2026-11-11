//! `--normalize` through the binary: quiet on success, a note when the ceiling stops it short
//! of the target or when there is nothing to measure.

use crossfade_cli::normalize_note;
use crossfade_graph::Normalization;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("crossfade-norm-{name}-{}", std::process::id()));
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

fn crossfade(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_crossfade"))
        .args(args)
        .output()
        .unwrap()
}

/// Mono 16-bit 48 kHz WAV.
fn write_wav(path: &str, samples: &[i16]) {
    let data = u32::try_from(samples.len() * 2).unwrap();
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&48_000u32.to_le_bytes());
    v.extend_from_slice(&96_000u32.to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        v.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, v).unwrap();
}

fn tone(n: usize, amplitude: f64) -> Vec<i16> {
    (0..n)
        .map(|i| (amplitude * 32767.0 * (i as f64 * 0.13).sin()) as i16)
        .collect()
}

#[test]
fn notes_say_what_happened() {
    let quiet = Normalization {
        measured_lufs: Some(-30.0),
        target_lufs: -16.0,
        gain_db: 14.0,
        limited_by_ceiling: false,
        ceiling_dbtp: -1.0,
    };
    assert_eq!(normalize_note(&quiet), None);
    let limited = Normalization {
        gain_db: 9.5,
        limited_by_ceiling: true,
        ..quiet
    };
    assert_eq!(
        normalize_note(&limited).as_deref(),
        Some(
            "normalised to -20.5 LUFS, 4.5 LU short of -16 LUFS: the true peak may not pass \
             -1 dBTP (lower the target or raise --true-peak)"
        )
    );
    let silent = Normalization {
        measured_lufs: None,
        gain_db: 0.0,
        ..quiet
    };
    assert_eq!(
        normalize_note(&silent).as_deref(),
        Some("nothing to normalise (silence, or under 400 ms): the level is left as it is")
    );
}

#[test]
fn the_binary_is_quiet_on_success_and_notes_a_short_fall() {
    let d = Dir::new("bin");
    let quiet = d.path("quiet.wav");
    write_wav(&quiet, &tone(96_000, 0.05));
    let o = crossfade(&[
        "convert",
        &quiet,
        "-o",
        &d.path("a.flac"),
        "--normalize",
        "-16LUFS",
    ]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(
        o.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let o = crossfade(&[
        "convert",
        &quiet,
        "-o",
        &d.path("b.flac"),
        "--normalize",
        "0LUFS",
        "--true-peak",
        "-6dBTP",
    ]);
    assert_eq!(o.status.code(), Some(0));
    let err = String::from_utf8(o.stderr).unwrap();
    assert!(err.starts_with("crossfade: note: normalised to "), "{err}");
    assert!(err.contains("short of 0 LUFS"), "{err}");
    let silent = d.path("silent.wav");
    write_wav(&silent, &vec![0; 48_000]);
    let o = crossfade(&[
        "convert",
        &silent,
        "-o",
        &d.path("c.wav"),
        "--normalize",
        "-16LUFS",
    ]);
    assert_eq!(o.status.code(), Some(0));
    assert!(
        String::from_utf8(o.stderr)
            .unwrap()
            .contains("nothing to normalise")
    );
}
