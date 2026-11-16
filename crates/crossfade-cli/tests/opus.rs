//! Ogg Opus through the real binary: probe, conversion to FLAC (16-bit, with the lossy note)
//! and to float WAV with the decoder's own samples, pre-skip dropped and the end trimmed;
//! stream copy into Ogg Opus, and the refusal of what would need an Opus encoder.

use crossfade_opus::Decoder;
use crossfade_opus::testing::ogg::{self as w, OggLayout};
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
        let p = std::env::temp_dir().join(format!("crossfade-opus-{test}-{}", std::process::id()));
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

const PRE_SKIP: usize = 312;
const END_TRIM: usize = 200;

/// The file, its packets and its audible frames.
fn ogg() -> (Vec<u8>, Vec<Vec<u8>>, u64) {
    let mut audio = w::packets(9, true, 10, 50, 1);
    audio.extend(w::packets(15, true, 10, 70, 2));
    audio.extend(w::packets(31, true, 10, 90, 3));
    let bytes = w::ogg_file(
        &w::opus_head(2, PRE_SKIP as u16, 0, None),
        &w::opus_tags("crossfade test", &["TITLE=Tone"]),
        &audio,
        &OggLayout {
            end_trim: END_TRIM as i64,
            ..OggLayout::default()
        },
    );
    (bytes, audio, (30 * 960 - PRE_SKIP - END_TRIM) as u64)
}

fn data_chunk(wav: &[u8]) -> &[u8] {
    let at = wav.windows(4).position(|w| w == b"data").unwrap();
    let len = u32::from_le_bytes(wav[at + 4..at + 8].try_into().unwrap()) as usize;
    &wav[at + 8..at + 8 + len]
}

#[test]
fn probe_shows_an_ogg_opus_file() {
    let d = Dir::new("probe");
    let (bytes, _, frames) = ogg();
    std::fs::write(d.path("in.opus"), &bytes).unwrap();
    let out = run(&["probe", &d.path("in.opus")]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let shown = text(&out.stdout);
    assert!(shown.starts_with("Input: ogg, "), "{shown}");
    assert!(
        shown.contains("audio, opus, 48000 Hz, stereo, f32, "),
        "{shown}"
    );
    assert!(shown.contains(&format!("({frames} frames)")), "{shown}");
    assert!(shown.contains("TITLE"), "{shown}");
}

#[test]
fn opus_converts_to_16_bit_flac_with_a_note_and_to_float_exactly() {
    let d = Dir::new("convert");
    let (bytes, audio, frames) = ogg();
    std::fs::write(d.path("in.opus"), &bytes).unwrap();

    let out = run(&["convert", &d.path("in.opus"), "-o", &d.path("out.flac")]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let said = text(&out.stdout) + &text(&out.stderr);
    assert!(
        said.contains("opus has no bit depth: writing 16-bit; choose --sample-format to change"),
        "{said}"
    );
    let out = run(&["probe", &d.path("out.flac")]);
    let shown = text(&out.stdout);
    assert!(shown.contains("flac, 48000 Hz, stereo, s16"), "{shown}");
    assert!(shown.contains(&format!("({frames} frames)")), "{shown}");

    let out = run(&[
        "convert",
        &d.path("in.opus"),
        "-o",
        &d.path("of.wav"),
        "--sample-format",
        "f32",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let mut dec = Decoder::new(2).unwrap();
    let mut pcm = vec![];
    for p in &audio {
        dec.decode(Some(p), &mut pcm).unwrap();
    }
    let want: Vec<u8> = pcm[PRE_SKIP * 2..(PRE_SKIP + frames as usize) * 2]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let wav = std::fs::read(d.path("of.wav")).unwrap();
    assert!(data_chunk(&wav) == want, "float output differs");
}

#[test]
fn opus_is_copied_into_ogg_opus_and_decodes_the_same() {
    let d = Dir::new("copy");
    let (bytes, _, frames) = ogg();
    std::fs::write(d.path("in.opus"), &bytes).unwrap();
    for name in ["out.opus", "out.ogg"] {
        let out = run(&[
            "convert",
            &d.path("in.opus"),
            "-o",
            &d.path(name),
            "--codec",
            "copy",
        ]);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    }
    // The same stream whatever the extension, and not the source's pages or vendor.
    let copy = std::fs::read(d.path("out.opus")).unwrap();
    assert!(copy == std::fs::read(d.path("out.ogg")).unwrap());
    assert!(copy != bytes);

    let probe = |name: &str| text(&run(&["probe", &d.path(name)]).stdout);
    let shown = probe("out.opus");
    assert!(
        shown.contains("audio, opus, 48000 Hz, stereo, f32, "),
        "{shown}"
    );
    assert!(shown.contains(&format!("({frames} frames)")), "{shown}");
    assert!(shown.contains("TITLE"), "{shown}");

    for (from, to) in [("in.opus", "a.wav"), ("out.opus", "b.wav")] {
        let out = run(&[
            "convert",
            &d.path(from),
            "-o",
            &d.path(to),
            "--sample-format",
            "f32",
        ]);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    }
    let (a, b) = (
        std::fs::read(d.path("a.wav")).unwrap(),
        std::fs::read(d.path("b.wav")).unwrap(),
    );
    assert_eq!(data_chunk(&a).len() as u64, frames * 8);
    assert!(a == b, "the copy decodes differently");

    // A copy of the copy is the copy.
    let out = run(&[
        "convert",
        &d.path("out.opus"),
        "-o",
        &d.path("again.opus"),
        "--codec",
        "copy",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(copy == std::fs::read(d.path("again.opus")).unwrap());
}

#[test]
fn pcm_is_encoded_to_opus_by_extension_or_by_name() {
    let d = Dir::new("encode");
    let (bytes, _, _) = ogg();
    std::fs::write(d.path("in.opus"), &bytes).unwrap();
    let out = run(&["convert", &d.path("in.opus"), "-o", &d.path("pcm.wav")]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    for (args, output) in [
        (vec![], "out.opus"),
        (vec!["--codec", "opus"], "named.ogg"),
        (vec!["--channels", "mono"], "mono.opus"),
    ] {
        let mut all = vec![
            "convert".to_string(),
            d.path("pcm.wav"),
            "-o".into(),
            d.path(output),
        ];
        all.extend(args.iter().map(|a| a.to_string()));
        let all: Vec<&str> = all.iter().map(String::as_str).collect();
        let out = run(&all);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{output}: {}",
            text(&out.stderr)
        );
        let probe = run(&["probe", &d.path(output)]);
        let report = text(&probe.stdout);
        assert!(
            report.contains("opus") && report.contains("48000"),
            "{output}: {report}"
        );
    }
    // What Opus cannot take is said by name, and leaves no file.
    let out = run(&[
        "convert",
        &d.path("pcm.wav"),
        "-o",
        &d.path("no.opus"),
        "--sample-format",
        "s24",
    ]);
    assert_eq!(out.status.code(), Some(1));
    let err = text(&out.stderr);
    assert!(
        err.contains("opus has no sample format; leave out --sample-format"),
        "{err}"
    );
    assert!(!std::path::Path::new(&d.path("no.opus")).exists());
    // PCM has no Opus packets to copy.
    let out = run(&[
        "convert",
        &d.path("pcm.wav"),
        "-o",
        &d.path("copied.opus"),
        "--codec",
        "copy",
    ]);
    assert_eq!(out.status.code(), Some(1));
    let err = text(&out.stderr);
    assert!(
        err.contains("cannot copy pcm_s16le into ogg (ogg holds flac or opus)"),
        "{err}"
    );
}

#[test]
fn bitrate_takes_bits_or_kilobits_and_only_for_opus() {
    let d = Dir::new("bitrate");
    let (bytes, _, _) = ogg();
    std::fs::write(d.path("in.opus"), &bytes).unwrap();
    let out = run(&["convert", &d.path("in.opus"), "-o", &d.path("pcm.wav")]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let size = |name: &str, bitrate: &str| {
        let out = run(&[
            "convert",
            &d.path("pcm.wav"),
            "-o",
            &d.path(name),
            "--bitrate",
            bitrate,
        ]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{bitrate}: {}",
            text(&out.stderr)
        );
        std::fs::metadata(d.path(name)).unwrap().len()
    };
    let (a, b, c) = (
        size("a.opus", "32k"),
        size("b.opus", "32000"),
        size("c.opus", "128kbps"),
    );
    assert_eq!(a, b);
    assert!(c > 2 * a, "{c} against {a}");
    for (output, bitrate, why) in [
        ("d.opus", "3k", "opus takes 6000 to 510000 bits per second"),
        ("d.opus", "fast", "a bitrate such as 96k or 128000"),
        ("d.flac", "96k", "--bitrate applies to opus only"),
    ] {
        let out = run(&[
            "convert",
            &d.path("pcm.wav"),
            "-o",
            &d.path(output),
            "--bitrate",
            bitrate,
        ]);
        assert_ne!(out.status.code(), Some(0), "{bitrate}");
        assert!(
            text(&out.stderr).contains(why),
            "{bitrate}: {}",
            text(&out.stderr)
        );
    }
}

#[test]
fn tags_joins_trims_and_normalising_all_reach_an_opus_file() {
    let d = Dir::new("paths");
    let (bytes, _, _) = ogg();
    std::fs::write(d.path("in.opus"), &bytes).unwrap();
    let ok = |args: &[&str]| {
        let out = run(args);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{args:?}: {}",
            text(&out.stderr)
        );
        text(&out.stderr)
    };
    // The source's tags travel through FLAC and back into Opus.
    ok(&["convert", &d.path("in.opus"), "-o", &d.path("a.flac")]);
    ok(&["convert", &d.path("a.flac"), "-o", &d.path("tagged.opus")]);
    let report = text(&run(&["probe", &d.path("tagged.opus")]).stdout);
    assert!(report.contains("Tone"), "{report}");

    let frames = |name: &str| -> u64 {
        let json = text(&run(&["probe", &d.path(name), "--json"]).stdout);
        let at = json.find("\"frames\":").unwrap_or_else(|| panic!("{json}"));
        json[at + 9..]
            .trim_start()
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .unwrap()
    };
    let one = frames("tagged.opus");
    // A join is as long as its parts.
    ok(&[
        "concat",
        &d.path("a.flac"),
        &d.path("a.flac"),
        "-o",
        &d.path("twice.opus"),
    ]);
    assert_eq!(frames("twice.opus"), 2 * one);
    // A trim to 50 ms, at Opus's 48 kHz.
    ok(&[
        "convert",
        &d.path("a.flac"),
        "-o",
        &d.path("cut.opus"),
        "--filter",
        "trim start=0.1s duration=0.05s",
    ]);
    assert!(one > 12_000, "{one}");
    assert_eq!(frames("cut.opus"), 2_400);
    // Normalising measures first, then encodes at the gain found.
    ok(&[
        "convert",
        &d.path("a.flac"),
        "-o",
        &d.path("loud.opus"),
        "--filter",
        "normalize -20LUFS",
    ]);
    let json = text(&run(&["probe", &d.path("loud.opus"), "--loudness", "--json"]).stdout);
    let at = json
        .find("\"integrated_lufs\":")
        .unwrap_or_else(|| panic!("{json}"));
    let lufs: f64 = json[at + 18..]
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '-' || *c == '.')
        .collect::<String>()
        .parse()
        .unwrap();
    assert!((lufs + 20.0).abs() < 1.0, "{lufs} LUFS");
}
