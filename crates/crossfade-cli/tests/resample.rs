//! Awkward rate pairs through the binary: the output header says the new rate and exactly
//! `ceil(N * out / in)` frames, and the file reads back (FLAC checks its MD5 and total).

use std::path::PathBuf;
use std::process::{Command, Output};

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("crossfade-rates-{name}-{}", std::process::id()));
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

/// Stereo 16-bit WAV of a tone at `rate`, `frames` long.
fn write_wav(path: &str, rate: u32, frames: u32) {
    let data = frames * 4;
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * 4).to_le_bytes());
    v.extend_from_slice(&4u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        let s = (8000.0 * (f64::from(i) * 0.05).sin()) as i16;
        v.extend_from_slice(&s.to_le_bytes());
        v.extend_from_slice(&(-s).to_le_bytes());
    }
    std::fs::write(path, v).unwrap();
}

fn json_field(json: &str, key: &str) -> u64 {
    let at = json.find(&format!("\"{key}\": ")).unwrap() + key.len() + 4;
    json[at..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap()
}

#[test]
fn awkward_rates_convert_with_the_right_header_and_length() {
    let d = Dir::new("awkward");
    for (from, to, frames) in [
        (44_100u32, 44_101u32, 44_100u32),
        (8_000, 192_000, 8_000),
        (192_000, 8_000, 192_001),
    ] {
        let input = d.path(&format!("in{from}.wav"));
        write_wav(&input, from, frames);
        let want = (u64::from(frames) * u64::from(to)).div_ceil(u64::from(from));
        for ext in ["wav", "flac"] {
            let out = d.path(&format!("{from}to{to}.{ext}"));
            let o = crossfade(&[
                "convert",
                &input,
                "-o",
                &out,
                "--sample-rate",
                &to.to_string(),
            ]);
            assert_eq!(
                o.status.code(),
                Some(0),
                "{}",
                String::from_utf8_lossy(&o.stderr)
            );
            let json = String::from_utf8(crossfade(&["probe", &out, "--json"]).stdout).unwrap();
            assert_eq!(json_field(&json, "sample_rate"), u64::from(to), "{out}");
            assert_eq!(json_field(&json, "frames"), want, "{from} -> {to} {ext}");
            let back = d.path(&format!("{from}to{to}.{ext}.wav"));
            let o = crossfade(&["convert", &out, "-o", &back]);
            assert_eq!(
                o.status.code(),
                Some(0),
                "{}",
                String::from_utf8_lossy(&o.stderr)
            );
            let json = String::from_utf8(crossfade(&["probe", &back, "--json"]).stdout).unwrap();
            assert_eq!(json_field(&json, "frames"), want, "read back from {ext}");
        }
    }
}
