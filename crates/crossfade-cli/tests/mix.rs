//! `probe` names speaker layouts, and `convert --channels stereo` mixes a 5.1 file down.

use std::process::Command;

/// A 5.1 `WAVE_FORMAT_EXTENSIBLE` file (mask 0x60F), 100 frames of 16-bit silence.
fn five_one_wav() -> Vec<u8> {
    let pcm = vec![0u8; 100 * 12];
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(4 + 8 + 40 + 8 + pcm.len() as u32).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&40u32.to_le_bytes());
    v.extend_from_slice(&0xFFFEu16.to_le_bytes());
    v.extend_from_slice(&6u16.to_le_bytes());
    v.extend_from_slice(&48000u32.to_le_bytes());
    v.extend_from_slice(&(48000u32 * 12).to_le_bytes());
    v.extend_from_slice(&12u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(&22u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(&0x60Fu32.to_le_bytes());
    v.extend_from_slice(&[
        1, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71,
    ]);
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    v.extend_from_slice(&pcm);
    v
}

fn run(dir: &std::path::Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_crossfade"))
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("crossfade-mix-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("six.wav"), five_one_wav()).unwrap();
    d
}

#[test]
fn probe_names_a_five_one_file_in_text_and_json() {
    let d = dir("probe");
    let text = run(&d, &["probe", "six.wav"]);
    assert!(text.contains("48000 Hz, 5.1, s16"), "{text}");
    let json = run(&d, &["probe", "six.wav", "--json"]);
    assert!(json.contains("\"layout\": \"5.1\""), "{json}");
    assert!(json.contains("\"schema\": 1"), "{json}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn convert_channels_stereo_mixes_the_file_down() {
    let d = dir("convert");
    run(
        &d,
        &[
            "convert",
            "six.wav",
            "-o",
            "two.flac",
            "--channels",
            "stereo",
        ],
    );
    let text = run(&d, &["probe", "two.flac"]);
    assert!(text.contains("48000 Hz, stereo"), "{text}");
    assert!(text.contains("(100 frames)"), "{text}");
    let _ = std::fs::remove_dir_all(&d);
}
