//! ALAC in `.m4a` through the real binary: probe, and conversions that keep every bit.

#[path = "../../crossfade-format/tests/support/mp4_signals.rs"]
mod mp4_signals;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../crossfade-format/tests/fixtures/mp4/{name}"))
}

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
        let p = std::env::temp_dir().join(format!("crossfade-mp4-{test}-{}", std::process::id()));
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

/// The `fmt ` valid bits (or bits per sample) and the `data` payload of a WAV file.
fn wav(bytes: &[u8]) -> (u16, u16, &[u8]) {
    assert_eq!(&bytes[..4], b"RIFF");
    let (mut fmt, mut data) = (None, None);
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = &bytes[pos + 8..pos + 8 + len];
        match id {
            b"fmt " => fmt = Some(body),
            b"data" => data = Some(body),
            _ => {}
        }
        pos += 8 + len + (len & 1);
    }
    let fmt = fmt.unwrap();
    let container = u16::from_le_bytes([fmt[14], fmt[15]]);
    let valid = if fmt.len() >= 20 && u16::from_le_bytes([fmt[0], fmt[1]]) == 0xFFFE {
        u16::from_le_bytes([fmt[18], fmt[19]])
    } else {
        container
    };
    (container, valid, data.unwrap())
}

/// The source samples as a WAV writer stores them: left-justified in whole bytes.
fn expected_pcm(signal: &mp4_signals::Signal) -> Vec<u8> {
    let width = usize::from(signal.bits).div_ceil(8);
    let shift = width as u32 * 8 - u32::from(signal.bits);
    signal
        .samples
        .iter()
        .flat_map(|s| (s << shift).to_le_bytes()[..width].to_vec())
        .collect()
}

#[test]
fn probe_shows_an_alac_m4a() {
    let out = run(&["probe", &fixture("alac-stereo16.m4a").to_string_lossy()]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let shown = text(&out.stdout);
    assert!(shown.starts_with("Input: mp4, 29133 bytes\n"), "{shown}");
    assert!(
        shown.contains("Stream 0: audio, alac, 44100 Hz, stereo, s16, 0.227 s (10007 frames)"),
        "{shown}"
    );
    let out = run(&[
        "probe",
        "--json",
        &fixture("alac-stereo20.m4a").to_string_lossy(),
    ]);
    assert_eq!(out.status.code(), Some(0));
    let json = text(&out.stdout);
    for field in [
        r#""format": "mp4""#,
        r#""codec": "alac""#,
        r#""sample_format": "s24""#,
        r#""bits_per_raw_sample": 20"#,
        r#""sample_rate": 48000"#,
        r#""channels": 2"#,
        r#""frames": 4001"#,
    ] {
        assert!(json.contains(field), "{field} in {json}");
    }
}

#[test]
fn alac_converts_to_wav_and_flac_bit_for_bit() {
    let dir = Dir::new("convert");
    for (file, signal) in [
        ("alac-stereo16.m4a", "stereo16"),
        ("alac-stereo20.m4a", "stereo20"),
        ("alac-stereo24.m4a", "stereo24"),
        ("alac-ch6.m4a", "ch6"),
    ] {
        let want = mp4_signals::signal(signal);
        let input = fixture(file).to_string_lossy().into_owned();
        let (direct, flac, back) = (dir.path("d.wav"), dir.path("f.flac"), dir.path("b.wav"));
        for args in [
            vec!["convert", "--force", input.as_str(), "-o", direct.as_str()],
            vec!["convert", "--force", input.as_str(), "-o", flac.as_str()],
            vec!["convert", "--force", flac.as_str(), "-o", back.as_str()],
        ] {
            let out = run(&args);
            assert_eq!(
                out.status.code(),
                Some(0),
                "{args:?}: {}",
                text(&out.stderr)
            );
        }
        for path in [&direct, &back] {
            let bytes = std::fs::read(path).unwrap();
            let (container, valid, data) = wav(&bytes);
            assert_eq!(valid, u16::from(want.bits), "{file}");
            assert_eq!(container, u16::from(want.bits).div_ceil(8) * 8, "{file}");
            assert!(data == expected_pcm(&want), "{file} via {path}");
        }
    }
}

#[test]
fn stream_copy_out_of_m4a_is_refused() {
    let dir = Dir::new("copy");
    let input = fixture("alac-stereo16.m4a").to_string_lossy().into_owned();
    for name in ["c.flac", "c.wav"] {
        let out = run(&["convert", &input, "--codec", "copy", "-o", &dir.path(name)]);
        assert_eq!(out.status.code(), Some(1));
        let err = text(&out.stderr);
        assert!(err.contains("cannot copy alac into"), "{err}");
    }
}
