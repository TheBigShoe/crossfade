//! Ctrl-C during `convert`: the real binary, a real SIGINT. Unix only: Windows has no
//! `kill`, and the in-process tests in the library cover the cancel path itself.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("crossfade-sig-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }

    fn listing(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(&self.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A mono 16-bit WAV claiming `frames` frames of silence, on a sparse file: it costs no
/// disk, yet FLAC at level 8 still has to encode every frame of it.
fn write_sparse_wav(path: &Path, frames: u64) {
    let data = u32::try_from(frames * 2).expect("under the 4 GiB WAV limit");
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&8000u32.to_le_bytes());
    v.extend_from_slice(&16_000u32.to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data.to_le_bytes());
    std::fs::write(path, v).unwrap();
    let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_len(44 + u64::from(data)).unwrap();
}

enum Attempt {
    /// Interrupted as intended.
    Interrupted,
    /// The conversion ended before the signal could matter; try a bigger input.
    TooFast,
}

fn attempt(frames: u64) -> Attempt {
    let dir = Dir::new(&frames.to_string());
    write_sparse_wav(&dir.0.join("in.wav"), frames);
    let mut child = Command::new(env!("CARGO_BIN_EXE_crossfade"))
        .args([
            "convert",
            "in.wav",
            "-o",
            "out.flac",
            "--compression-level",
            "8",
        ])
        .current_dir(&dir.0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // The hidden temp file proves the handler is installed and the engine is writing. The
    // bound only stops a hang; nothing asserts how long it took.
    let mut seen = false;
    for _ in 0..2000 {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if dir.listing().iter().any(|n| n.starts_with('.')) {
            seen = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    if !seen {
        let _ = child.kill();
        let _ = child.wait();
        return Attempt::TooFast;
    }
    let status = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success(), "kill -INT failed");
    let out = child.wait_with_output().unwrap();
    if out.status.code() == Some(0) {
        // It won the race and committed the output.
        return Attempt::TooFast;
    }
    assert_eq!(out.status.code(), Some(130), "{:?}", out.status);
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "crossfade: cancelled\n"
    );
    assert_eq!(dir.listing(), ["in.wav"]);
    Attempt::Interrupted
}

#[test]
fn sigint_cancels_the_conversion_and_cleans_up() {
    // Each step quadruples the work; the last is near the 4 GiB WAV ceiling.
    let mut frames = 1u64 << 24;
    while frames <= 1 << 30 {
        if matches!(attempt(frames), Attempt::Interrupted) {
            return;
        }
        frames *= 4;
    }
    panic!("the conversion always finished before the interrupt could land");
}
