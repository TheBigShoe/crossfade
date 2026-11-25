//! Writing Opus through the real binary: `.opus` and `--codec opus` give Ogg Opus that reads
//! back at the input's length (its rate brought to 48 kHz) and close to its signal, tags
//! survive, the bitrate decides the size, and what Opus cannot take is refused in plain words.

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

/// Success: stdout.
fn ok(args: &[&str]) -> String {
    let out = run(args);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{args:?}: {}",
        text(&out.stderr)
    );
    text(&out.stdout)
}

/// Success: stderr (notes and warnings).
fn ok_err(args: &[&str]) -> String {
    let out = run(args);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{args:?}: {}",
        text(&out.stderr)
    );
    text(&out.stderr)
}

fn refused(args: &[&str]) -> String {
    let out = run(args);
    assert_ne!(out.status.code(), Some(0), "{args:?} should fail");
    text(&out.stderr)
}

/// A unique scratch directory, removed on drop.
struct Dir(PathBuf);

impl Dir {
    fn new(test: &str) -> Self {
        let p =
            std::env::temp_dir().join(format!("crossfade-opuswrite-{test}-{}", std::process::id()));
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

/// A chirp (200 Hz up to 8 kHz), channel `c` a little lower, as interleaved floats.
fn chirp(rate: u32, channels: usize, frames: usize) -> Vec<f32> {
    let secs = (frames as f64 / f64::from(rate)).max(0.001);
    let mut v = Vec::with_capacity(frames * channels);
    for i in 0..frames {
        let t = i as f64 / f64::from(rate);
        for c in 0..channels {
            let f1 = 8_000.0 - 1_500.0 * c as f64;
            let phase =
                2.0 * std::f64::consts::PI * (200.0 * t + (f1 - 200.0) * t * t / (2.0 * secs));
            v.push((0.4 * phase.sin()) as f32);
        }
    }
    v
}

/// A 16-bit WAV file, with an optional LIST/INFO title after the data.
fn wav16(rate: u32, channels: usize, samples: &[f32], title: Option<&str>) -> Vec<u8> {
    let data: Vec<u8> = samples
        .iter()
        .flat_map(|&x| ((x * 32767.0).round() as i16).to_le_bytes())
        .collect();
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF\0\0\0\0WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&(channels as u16).to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
    b.extend_from_slice(&(channels as u16 * 2).to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(&data);
    if let Some(t) = title {
        let mut v = t.as_bytes().to_vec();
        v.push(0);
        if v.len() % 2 == 1 {
            v.push(0);
        }
        let mut info = b"INFO".to_vec();
        info.extend_from_slice(b"INAM");
        info.extend_from_slice(&(v.len() as u32).to_le_bytes());
        info.extend_from_slice(&v);
        b.extend_from_slice(b"LIST");
        b.extend_from_slice(&(info.len() as u32).to_le_bytes());
        b.extend_from_slice(&info);
    }
    let riff = (b.len() - 8) as u32;
    b[4..8].copy_from_slice(&riff.to_le_bytes());
    b
}

/// The samples of a 32-bit float WAV file written by the tool.
fn read_f32(path: &str) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap();
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        if &bytes[pos..pos + 4] == b"data" {
            return bytes[pos + 8..pos + 8 + len]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes(*c))
                .collect();
        }
        pos += 8 + len + (len & 1);
    }
    panic!("no data chunk");
}

fn snr_db(a: &[f32], b: &[f32]) -> f64 {
    let sig: f64 = a.iter().map(|&x| f64::from(x).powi(2)).sum();
    let err: f64 = a
        .iter()
        .zip(b)
        .map(|(&x, &y)| f64::from(x - y).powi(2))
        .sum();
    10.0 * (sig / err.max(1e-30)).log10()
}

/// The lag (in frames, within 400) at which `got` best follows `want`, on the first channel.
fn best_lag(want: &[f32], got: &[f32], channels: usize) -> i64 {
    let frames = want.len() / channels;
    let score = |lag: i64| -> f64 {
        (0..frames as i64)
            .filter(|&i| (0..got.len() as i64 / channels as i64).contains(&(i + lag)))
            .map(|i| {
                f64::from(want[i as usize * channels])
                    * f64::from(got[(i + lag) as usize * channels])
            })
            .sum()
    };
    (-400..=400)
        .max_by(|&a, &b| score(a).total_cmp(&score(b)))
        .unwrap()
}

fn convert_in(dir: &Dir, rate: u32, channels: usize, frames: usize) -> (String, Vec<f32>) {
    let pcm = chirp(rate, channels, frames);
    let wav = dir.path("in.wav");
    std::fs::write(&wav, wav16(rate, channels, &pcm, None)).unwrap();
    (wav, pcm)
}

#[test]
fn a_wav_written_as_opus_reads_back_as_long_and_close_to_the_source() {
    let dir = Dir::new("roundtrip");
    for (channels, bitrate) in [(2usize, "128k"), (1, "96k"), (2, "32k"), (2, "256k")] {
        let frames = 30_000;
        let (wav, pcm) = convert_in(&dir, 48_000, channels, frames);
        let (opus, back) = (dir.path("o.opus"), dir.path("back.wav"));
        ok(&[
            "convert",
            "--force",
            &wav,
            "-o",
            &opus,
            "--bitrate",
            bitrate,
        ]);
        let shown = ok(&["probe", &opus]);
        assert!(shown.contains("opus, 48000 Hz"), "{shown}");
        ok(&[
            "convert",
            "--force",
            &opus,
            "-o",
            &back,
            "--sample-format",
            "f32",
        ]);
        let got = read_f32(&back);
        assert_eq!(got.len(), pcm.len(), "{channels} ch {bitrate}");
        assert_eq!(best_lag(&pcm, &got, channels), 0, "{channels} ch {bitrate}");
        // Measured 38 dB (mono) and 28 dB (stereo) at the default rates; 32 kb/s keeps the
        // shape (a wideband chirp is a hard case for a narrow band) but not the detail.
        let floor = if bitrate == "32k" { 4.0 } else { 15.0 };
        let snr = snr_db(&pcm, &got);
        assert!(snr > floor, "{channels} ch {bitrate}: {snr:.1} dB");
    }
}

#[test]
fn a_44_1_khz_source_is_resampled_keeps_its_seconds_and_says_so() {
    let dir = Dir::new("rate");
    let (wav, _) = convert_in(&dir, 44_100, 2, 44_100);
    let (opus, back) = (dir.path("o.opus"), dir.path("back.wav"));
    let err = ok_err(&["convert", &wav, "-o", &opus]);
    assert!(
        err.contains("opus is always 48 kHz: resampling from 44100 Hz"),
        "{err}"
    );
    ok(&["convert", &opus, "-o", &back, "--sample-format", "f32"]);
    let frames = read_f32(&back).len() / 2;
    assert!(frames.abs_diff(48_000) <= 1, "{frames}");
}

#[test]
fn a_very_short_file_keeps_its_length() {
    let dir = Dir::new("short");
    for frames in [1usize, 300, 648, 649] {
        let (wav, _) = convert_in(&dir, 48_000, 2, frames);
        let (opus, back) = (dir.path("o.opus"), dir.path("back.wav"));
        ok(&["convert", "--force", &wav, "-o", &opus]);
        ok(&[
            "convert",
            "--force",
            &opus,
            "-o",
            &back,
            "--sample-format",
            "f32",
        ]);
        assert_eq!(read_f32(&back).len(), frames * 2, "{frames}");
    }
}

#[test]
fn tags_survive_into_opus() {
    let dir = Dir::new("tags");
    let wav = dir.path("in.wav");
    std::fs::write(
        &wav,
        wav16(48_000, 2, &chirp(48_000, 2, 9_600), Some("Tone")),
    )
    .unwrap();
    let opus = dir.path("o.opus");
    ok(&["convert", &wav, "-o", &opus]);
    assert!(ok(&["probe", &opus]).contains("TITLE: Tone"));
}

#[test]
fn the_bitrate_sets_the_size_and_constant_bitrate_holds_it() {
    let dir = Dir::new("size");
    let (wav, _) = convert_in(&dir, 48_000, 2, 96_000);
    let size = |extra: &[&str]| {
        let opus = dir.path("o.opus");
        let mut args = vec!["convert", "--force", wav.as_str(), "-o", opus.as_str()];
        args.extend_from_slice(extra);
        ok(&args);
        std::fs::metadata(&opus).unwrap().len()
    };
    let (low, mid, high) = (
        size(&["--bitrate", "32k"]),
        size(&["--bitrate", "64kbps"]),
        size(&["--bitrate", "256000"]),
    );
    assert!(low < mid && mid < high, "{low} {mid} {high}");
    // Two seconds at 64 kb/s constant: 16000 bytes of packets plus headers and page framing.
    let cbr = size(&["--bitrate", "64k", "--cbr"]);
    assert!((16_000..17_500).contains(&cbr), "{cbr}");
}

#[test]
fn codec_opus_puts_opus_in_ogg_and_ogg_stays_flac() {
    let dir = Dir::new("ogg");
    let (wav, _) = convert_in(&dir, 48_000, 2, 9_600);
    for out in ["a.ogg", "a.oga"] {
        let path = dir.path(out);
        ok(&["convert", &wav, "-o", &path, "--codec", "opus"]);
        assert!(ok(&["probe", &path]).contains("opus,"), "{out}");
        ok(&["convert", "--force", &wav, "-o", &path]);
        assert!(ok(&["probe", &path]).contains("flac,"), "{out}");
    }
}

#[test]
fn processing_and_joins_work_into_opus() {
    let dir = Dir::new("processing");
    let (wav, _) = convert_in(&dir, 44_100, 2, 44_100);
    let opus = dir.path("o.opus");
    ok(&[
        "convert",
        &wav,
        "-o",
        &opus,
        "--normalize",
        "-16LUFS",
        "--true-peak",
        "-1dBTP",
    ]);
    ok(&[
        "convert",
        "--force",
        &wav,
        "-o",
        &opus,
        "--gain",
        "-6dB",
        "--channels",
        "mono",
    ]);
    assert!(ok(&["probe", &opus]).contains("mono"));
    let (wav2, joined, back) = (dir.path("two.wav"), dir.path("j.opus"), dir.path("j.wav"));
    std::fs::write(&wav2, wav16(48_000, 2, &chirp(48_000, 2, 24_000), None)).unwrap();
    ok(&["concat", &wav, &wav2, "-o", &joined]);
    ok(&["convert", &joined, "-o", &back, "--sample-format", "f32"]);
    let frames = read_f32(&back).len() / 2;
    assert!(frames.abs_diff(48_000 + 24_000) <= 1, "{frames}");
}

#[test]
fn the_thread_count_never_changes_the_bytes_and_neither_does_a_second_run() {
    let dir = Dir::new("threads");
    let (wav, _) = convert_in(&dir, 44_100, 2, 60_000);
    let mut files = Vec::new();
    for threads in ["1", "2", "4", "auto", "1"] {
        let opus = dir.path(&format!("t{threads}-{}.opus", files.len()));
        ok(&["convert", &wav, "-o", &opus, "--threads", threads]);
        files.push(std::fs::read(&opus).unwrap());
    }
    assert!(files.windows(2).all(|w| w[0] == w[1]));
}

#[test]
fn what_opus_cannot_take_is_refused_in_plain_words() {
    let dir = Dir::new("refusals");
    let (wav, _) = convert_in(&dir, 48_000, 2, 9_600);
    let opus = dir.path("o.opus");
    let e = refused(&["convert", &wav, "-o", &opus, "--bitrate", "fast"]);
    assert!(
        e.contains("expected a bitrate such as 128k, 96000 or 64kbps"),
        "{e}"
    );
    let e = refused(&["convert", &wav, "-o", &opus, "--bitrate", "128"]);
    assert!(
        e.contains("outside opus's range") && e.contains("12000 to 512000"),
        "{e}"
    );
    let e = refused(&[
        "convert",
        &wav,
        "-o",
        &dir.path("o.flac"),
        "--bitrate",
        "128k",
    ]);
    assert!(e.contains("--bitrate applies to opus only"), "{e}");
    let e = refused(&["convert", &wav, "-o", &opus, "--sample-rate", "44100"]);
    assert!(e.contains("opus is always 48 kHz"), "{e}");
    let e = refused(&["convert", &wav, "-o", &opus, "--sample-format", "s16"]);
    assert!(e.contains("opus has no bit depth"), "{e}");
    let e = refused(&["convert", &wav, "-o", &dir.path("o.wav"), "--codec", "opus"]);
    assert!(e.contains("codec opus cannot be written into wav"), "{e}");
    // Six unlabelled channels are not Vorbis's 5.1: refused up front, with the advice that works.
    let six = dir.path("six.wav");
    std::fs::write(&six, wav16(48_000, 6, &chirp(48_000, 6, 4_800), None)).unwrap();
    let e = refused(&["convert", &six, "-o", &opus]);
    assert!(
        e.contains("opus stores 6 channels in one layout only (5.1-back)")
            && e.contains("--channels stereo"),
        "{e}"
    );
    ok(&["convert", &six, "-o", &opus, "--channels", "stereo"]);
    // And nine channels have no layout at all.
    let nine = dir.path("nine.wav");
    std::fs::write(&nine, wav16(48_000, 9, &chirp(48_000, 9, 4_800), None)).unwrap();
    let e = refused(&["convert", &nine, "-o", &dir.path("n.opus")]);
    assert!(e.contains("opus holds at most 8 channels, got 9"), "{e}");
}

/// A 16-bit WAVE_FORMAT_EXTENSIBLE file with a channel mask.
fn wav_mask(rate: u32, channels: usize, mask: u32, samples: &[f32]) -> Vec<u8> {
    let data: Vec<u8> = samples
        .iter()
        .flat_map(|&x| ((x * 32767.0).round() as i16).to_le_bytes())
        .collect();
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF\0\0\0\0WAVEfmt ");
    b.extend_from_slice(&40u32.to_le_bytes());
    b.extend_from_slice(&0xFFFEu16.to_le_bytes());
    b.extend_from_slice(&(channels as u16).to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
    b.extend_from_slice(&(channels as u16 * 2).to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(&22u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(&mask.to_le_bytes());
    // The PCM sub-format GUID.
    b.extend_from_slice(&[
        1, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71,
    ]);
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(&data);
    let riff = (b.len() - 8) as u32;
    b[4..8].copy_from_slice(&riff.to_le_bytes());
    b
}

/// How much of channel `c` (of `ch`) lies at `hz`, by the Goertzel recurrence.
fn tone_at(x: &[f32], ch: usize, c: usize, hz: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI * hz / 48_000.0;
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for f in x.chunks_exact(ch) {
        let s = f64::from(f[c]) + 2.0 * w.cos() * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    (s1 * s1 + s2 * s2 - 2.0 * w.cos() * s1 * s2)
        .max(0.0)
        .sqrt()
        / (x.len() / ch) as f64
}

#[test]
fn surround_comes_back_with_every_channel_in_its_place() {
    let dir = Dir::new("surround");
    // 5.1 with back surrounds (FL FR FC LFE BL BR) and 7.1 (those, then SL SR): the WAVE order.
    for (channels, mask, name) in [(6usize, 0x3Fu32, "5.1-back"), (8, 0x63F, "7.1")] {
        // A different tone on each channel; the LFE (index 3) a low one.
        let tones: Vec<f64> = (0..channels)
            .map(|c| {
                if c == 3 {
                    90.0
                } else {
                    700.0 + 530.0 * c as f64
                }
            })
            .collect();
        let frames = 24_000usize;
        let pcm: Vec<f32> = (0..frames)
            .flat_map(|i| {
                tones.iter().map(move |hz| {
                    (0.2 * (2.0 * std::f64::consts::PI * hz * i as f64 / 48_000.0).sin()) as f32
                })
            })
            .collect();
        let wav = dir.path("in.wav");
        std::fs::write(&wav, wav_mask(48_000, channels, mask, &pcm)).unwrap();
        let (opus, back) = (dir.path("s.opus"), dir.path("back.wav"));
        ok(&["convert", "--force", &wav, "-o", &opus]);
        let shown = ok(&["probe", &opus]);
        assert!(
            shown.contains("opus, 48000 Hz") && shown.contains(name),
            "{shown}"
        );
        ok(&[
            "convert",
            "--force",
            &opus,
            "-o",
            &back,
            "--sample-format",
            "f32",
        ]);
        let got = read_f32(&back);
        assert_eq!(got.len(), pcm.len(), "{name}");
        for c in 0..channels {
            let own = tone_at(&got, channels, c, tones[c]);
            assert!(own > 0.03, "{name}: channel {c} lost its tone ({own})");
            for (d, hz) in tones.iter().enumerate().filter(|(d, _)| *d != c) {
                let other = tone_at(&got, channels, c, *hz);
                assert!(
                    other * 10.0 < own,
                    "{name}: channel {c} carries channel {d}'s tone ({other} vs {own})"
                );
            }
        }
    }
}

#[test]
fn a_surround_bitrate_is_taken_for_the_whole_stream() {
    let dir = Dir::new("surround-rate");
    let pcm = chirp(48_000, 6, 96_000);
    let wav = dir.path("in.wav");
    std::fs::write(&wav, wav_mask(48_000, 6, 0x3F, &pcm)).unwrap();
    let size = |extra: &[&str]| {
        let opus = dir.path("s.opus");
        let mut args = vec!["convert", "--force", wav.as_str(), "-o", opus.as_str()];
        args.extend_from_slice(extra);
        ok(&args);
        std::fs::metadata(&opus).unwrap().len()
    };
    let (low, high) = (size(&["--bitrate", "96k"]), size(&["--bitrate", "512k"]));
    assert!(low * 2 < high, "{low} {high}");
    let e = refused(&[
        "convert",
        &wav,
        "-o",
        &dir.path("x.opus"),
        "--bitrate",
        "30k",
    ]);
    assert!(e.contains("36000 to 1536000"), "{e}");
}

#[test]
fn the_help_names_opus_and_the_bitrate() {
    let out = ok(&["convert", "--help"]);
    for want in ["--bitrate", "--cbr", "opus", ".opus"] {
        assert!(out.contains(want), "{want}");
    }
}
