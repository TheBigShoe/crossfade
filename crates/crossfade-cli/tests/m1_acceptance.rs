//! Milestone 1 acceptance: a WAV file goes through probe and convert, driven
//! through the real binary. The helpers do not use the engine, so they can
//! judge it.
use crossfade_core::{AudioSpec, ChannelLayout, SampleFormat};
use crossfade_testkit::{pcm, signal};
use std::path::PathBuf;
use std::process::{Command, Output};

/// Canonical 44-byte-header PCM WAV (format tag 1, or 3 for float).
fn wav_bytes(spec: &AudioSpec, pcm: &[u8]) -> Vec<u8> {
    let channels = spec.layout().channels();
    let bytes_per_frame = u32::try_from(spec.bytes_per_frame()).unwrap();
    let tag: u16 = if spec.format().is_float() { 3 } else { 1 };
    let data_len = u32::try_from(pcm.len()).unwrap();
    let mut v = Vec::with_capacity(44 + pcm.len());
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&tag.to_le_bytes());
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&spec.sample_rate().to_le_bytes());
    v.extend_from_slice(&(spec.sample_rate() * bytes_per_frame).to_le_bytes());
    v.extend_from_slice(&u16::try_from(bytes_per_frame).unwrap().to_le_bytes());
    v.extend_from_slice(&u16::try_from(spec.format().bits()).unwrap().to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    v.extend_from_slice(pcm);
    v
}

/// Walks the RIFF chunks and returns the `data` payload.
fn data_chunk(bytes: &[u8]) -> &[u8] {
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = pos + 8;
        if id == b"data" {
            return &bytes[body..body + len];
        }
        // Chunks are padded to even length.
        pos = body + len + (len & 1);
    }
    panic!("no data chunk");
}

/// A unique scratch directory, removed on drop.
struct Dir(PathBuf);

impl Dir {
    fn new(test: &str) -> Self {
        let p = std::env::temp_dir().join(format!("crossfade-m1-{test}-{}", std::process::id()));
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
        std::fs::read(self.path(name)).unwrap()
    }

    // Relative names and a fixed cwd keep the output stable.
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_crossfade"))
            .args(args)
            .current_dir(&self.0)
            .output()
            .unwrap()
    }

    fn listing(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
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

/// One second of 48 kHz stereo s16 sine: raw PCM and the WAV file.
fn sine() -> (Vec<u8>, Vec<u8>) {
    let spec = AudioSpec::new(48_000, SampleFormat::S16, ChannelLayout::stereo()).unwrap();
    let samples = signal::render(
        &signal::Signal::Sine {
            hz: 440.0,
            amplitude: 0.5,
        },
        &spec,
        48_000,
    )
    .unwrap();
    let pcm = pcm::encode_le(&samples, SampleFormat::S16);
    let wav = wav_bytes(&spec, &pcm);
    (pcm, wav)
}

#[test]
fn probe_json_snapshot() {
    let d = Dir::new("probe-json");
    d.write("sine.wav", &sine().1);
    let out = d.run(&["probe", "sine.wav", "--json"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stderr.is_empty(), "{}", text(&out.stderr));
    insta::assert_snapshot!(text(&out.stdout), @r#"
    {
      "schema": 1,
      "format": "wav",
      "size_bytes": 192044,
      "streams": [
        {
          "index": 0,
          "type": "audio",
          "codec": "pcm_s16le",
          "sample_format": "s16",
          "bits_per_raw_sample": null,
          "sample_rate": 48000,
          "channels": 2,
          "layout": "stereo",
          "frames": 48000,
          "duration_seconds": 1.0
        }
      ],
      "tags": []
    }
    "#);
}

#[test]
fn probe_human_output() {
    let d = Dir::new("probe-human");
    d.write("sine.wav", &sine().1);
    let out = d.run(&["probe", "sine.wav"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        text(&out.stdout),
        "Input: wav, 192044 bytes\n  Stream 0: audio, pcm_s16le, 48000 Hz, stereo, s16, 1.000 s (48000 frames)\n"
    );
}

#[test]
fn convert_round_trip_s24_and_back_is_bit_exact() {
    let d = Dir::new("round-trip");
    let (pcm, wav) = sine();
    d.write("sine.wav", &wav);
    for args in [
        [
            "convert",
            "sine.wav",
            "-o",
            "a.wav",
            "--sample-format",
            "s24",
        ],
        ["convert", "a.wav", "-o", "b.wav", "--sample-format", "s16"],
    ] {
        let out = d.run(&args);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        assert!(out.stdout.is_empty());
    }
    let probe = d.run(&["probe", "a.wav", "--json"]);
    let json = text(&probe.stdout);
    assert!(json.contains("\"codec\": \"pcm_s24le\""), "{json}");
    assert!(json.contains("\"frames\": 48000"), "{json}");
    assert_eq!(data_chunk(&d.read("b.wav")), &pcm[..]);
}

#[test]
fn truncated_input_fails_cleanly() {
    let d = Dir::new("truncated");
    let wav = sine().1;
    // The header still declares the full data size.
    d.write("cut.wav", &wav[..44 + 96_000]);
    let out = d.run(&["convert", "cut.wav", "-o", "out.wav"]);
    assert_eq!(out.status.code(), Some(1));
    let err = text(&out.stderr);
    assert!(err.contains("unexpected end of input"), "{err}");
    assert!(!d.path("out.wav").exists());
    assert_eq!(d.listing(), ["cut.wav"], "no temporary file may remain");
}

#[test]
fn overwrite_needs_force() {
    let d = Dir::new("overwrite");
    let wav = sine().1;
    d.write("sine.wav", &wav);
    d.write("out.wav", b"precious");
    let out = d.run(&["convert", "sine.wav", "-o", "out.wav"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(d.read("out.wav"), b"precious");
    let out = d.run(&["convert", "sine.wav", "-o", "out.wav", "--force"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(data_chunk(&d.read("out.wav")), data_chunk(&wav));
}

#[test]
fn emit_job_prints_json_and_writes_nothing() {
    let d = Dir::new("emit-job");
    d.write("sine.wav", &sine().1);
    let out = d.run(&[
        "convert",
        "sine.wav",
        "-o",
        "out.wav",
        "--sample-format",
        "s24",
        "--emit-job",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(d.listing(), ["sine.wav"]);
    insta::assert_snapshot!(text(&out.stdout), @r#"
    {
      "version": 1,
      "inputs": [
        {
          "path": "sine.wav"
        }
      ],
      "outputs": [
        {
          "path": "out.wav",
          "audio": {
            "sample_format": "s24"
          }
        }
      ]
    }
    "#);
}

#[test]
fn missing_input_names_the_file() {
    let d = Dir::new("missing");
    let out = d.run(&["convert", "nope.wav", "-o", "out.wav"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("nope.wav"));
}

#[test]
fn bad_sample_format_is_a_usage_error() {
    let d = Dir::new("bad-format");
    d.write("sine.wav", &sine().1);
    let out = d.run(&[
        "convert",
        "sine.wav",
        "-o",
        "out.wav",
        "--sample-format",
        "banana",
    ]);
    assert_eq!(out.status.code(), Some(2));
    // Guards against passing merely because the subcommand is unknown.
    assert!(
        text(&out.stderr).contains("banana"),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn probe_rejects_non_wav() {
    let d = Dir::new("not-wav");
    d.write("zeros.bin", &[0u8; 64]);
    let out = d.run(&["probe", "zeros.bin"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("unrecognised format"));
}
