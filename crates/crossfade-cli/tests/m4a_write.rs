//! Writing Apple Lossless into `.m4a` through the real binary: every depth and channel count
//! comes back bit for bit, the depth and tags survive, processing works as it does into FLAC,
//! a stream copy keeps the packets, and what ALAC cannot take is refused in plain words.

#[path = "../../crossfade-format/tests/support/mp4_signals.rs"]
mod mp4_signals;

use crossfade_opus::testing::ogg::{self as w, OggLayout};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../crossfade-format/tests/fixtures/mp4/{name}"))
        .to_string_lossy()
        .into_owned()
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_crossfade"))
        .args(args)
        .output()
        .unwrap()
}

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

fn refused(args: &[&str]) -> String {
    let out = run(args);
    assert_eq!(
        out.status.code(),
        Some(1),
        "{args:?}: {}",
        text(&out.stdout)
    );
    text(&out.stderr)
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// A unique scratch directory, removed on drop.
struct Dir(PathBuf);

impl Dir {
    fn new(test: &str) -> Self {
        let p = std::env::temp_dir().join(format!("crossfade-m4a-{test}-{}", std::process::id()));
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

/// The `data` payload of a WAV file.
fn wav_data(bytes: &[u8]) -> &[u8] {
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        if &bytes[pos..pos + 4] == b"data" {
            return &bytes[pos + 8..pos + 8 + len];
        }
        pos += 8 + len + (len & 1);
    }
    panic!("no data chunk");
}

fn read(path: &str) -> Vec<u8> {
    std::fs::read(path).unwrap()
}

/// The payload of the first top-level `mdat`.
fn mdat(bytes: &[u8]) -> &[u8] {
    let mut pos = 0;
    while pos + 8 <= bytes.len() {
        let size = u32::from_be_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
        let (head, size) = if size == 1 {
            (
                16,
                u64::from_be_bytes(bytes[pos + 8..pos + 16].try_into().unwrap()) as usize,
            )
        } else {
            (8, size)
        };
        if &bytes[pos + 4..pos + 8] == b"mdat" {
            return &bytes[pos + head..pos + size];
        }
        pos += size;
    }
    panic!("no mdat");
}

#[test]
fn every_depth_and_channel_count_comes_back_bit_for_bit() {
    let dir = Dir::new("roundtrip");
    for name in [
        "mono16", "stereo16", "stereo20", "stereo24", "stereo32", "ch3", "ch4", "ch5", "ch6",
        "ch7", "ch8", "noise16", "silence",
    ] {
        let signal = mp4_signals::signal(name);
        let (wav_in, m4a, wav_out) = (dir.path("in.wav"), dir.path("o.m4a"), dir.path("out.wav"));
        std::fs::write(&wav_in, signal.wav()).unwrap();
        ok(&["convert", "--force", &wav_in, "-o", &m4a]);
        ok(&["convert", "--force", &m4a, "-o", &wav_out]);
        assert!(
            wav_data(&read(&wav_out)) == wav_data(&signal.wav()),
            "{name}"
        );
        // The 20-bit file keeps its valid bits.
        let shown = ok(&["probe", &m4a]);
        assert_eq!(
            shown.contains("(20-bit)"),
            signal.bits == 20,
            "{name}: {shown}"
        );
    }
}

#[test]
fn flac_and_aiff_sources_reach_m4a_and_come_back_the_same() {
    let dir = Dir::new("sources");
    for name in ["stereo16", "stereo20", "stereo24", "stereo32"] {
        let signal = mp4_signals::signal(name);
        let wav_in = dir.path("in.wav");
        std::fs::write(&wav_in, signal.wav()).unwrap();
        for via in ["s.flac", "s.aiff"] {
            let (src, m4a, back) = (dir.path(via), dir.path("o.m4a"), dir.path("back.wav"));
            ok(&["convert", "--force", &wav_in, "-o", &src]);
            ok(&["convert", "--force", &src, "-o", &m4a]);
            ok(&["convert", "--force", &m4a, "-o", &back]);
            assert!(
                wav_data(&read(&back)) == wav_data(&signal.wav()),
                "{name} via {via}"
            );
        }
    }
}

#[test]
fn a_20_bit_flac_stays_20_bit_through_m4a() {
    let dir = Dir::new("depth");
    let signal = mp4_signals::signal("stereo20");
    let (wav_in, flac, m4a, back) = (
        dir.path("in.wav"),
        dir.path("a.flac"),
        dir.path("b.m4a"),
        dir.path("c.flac"),
    );
    std::fs::write(&wav_in, signal.wav()).unwrap();
    ok(&["convert", &wav_in, "-o", &flac]);
    assert!(ok(&["probe", &flac]).contains("s24 (20-bit)"));
    ok(&["convert", &flac, "-o", &m4a]);
    assert!(ok(&["probe", &m4a]).contains("alac, 48000 Hz, stereo, s24 (20-bit)"));
    ok(&["convert", &m4a, "-o", &back]);
    assert!(ok(&["probe", &back]).contains("s24 (20-bit)"));
    assert!(read(&back) == read(&flac));
}

#[test]
fn a_depth_alac_cannot_code_is_written_one_up_and_says_so() {
    let dir = Dir::new("twelve");
    let signal = mp4_signals::signal("mono16");
    // 12 real bits in a 16-bit container.
    let samples: Vec<i32> = signal.samples.iter().map(|s| (s >> 4) << 4).collect();
    let twelve = mp4_signals::Signal {
        bits: 12,
        samples: samples.iter().map(|s| s >> 4).collect(),
        ..signal
    };
    let (wav_in, m4a) = (dir.path("in.wav"), dir.path("o.m4a"));
    std::fs::write(&wav_in, twelve.wav()).unwrap();
    let out = run(&["convert", &wav_in, "-o", &m4a]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(
        text(&out.stderr).contains("alac cannot code 12-bit audio: writing 16-bit"),
        "{}",
        text(&out.stderr)
    );
    assert!(ok(&["probe", &m4a]).contains("alac, 22050 Hz, mono, s16,"));
}

#[test]
fn tags_survive_into_m4a_and_back() {
    let dir = Dir::new("tags");
    let mut wav = mp4_signals::signal("stereo16").wav();
    // A LIST/INFO chunk with a title and an artist, appended after `data`.
    let info: Vec<u8> = [(&b"INAM"[..], &b"Tone\0\0"[..]), (b"IART", b"Duo\0")]
        .iter()
        .flat_map(|(id, v)| {
            [
                id.to_vec(),
                (v.len() as u32).to_le_bytes().to_vec(),
                v.to_vec(),
            ]
            .concat()
        })
        .collect();
    let list = [
        b"LIST".to_vec(),
        (4 + info.len() as u32).to_le_bytes().to_vec(),
        b"INFO".to_vec(),
        info,
    ]
    .concat();
    wav.extend_from_slice(&list);
    let riff = (wav.len() - 8) as u32;
    wav[4..8].copy_from_slice(&riff.to_le_bytes());
    let (wav_in, m4a, flac) = (dir.path("in.wav"), dir.path("o.m4a"), dir.path("o.flac"));
    std::fs::write(&wav_in, wav).unwrap();
    ok(&["convert", &wav_in, "-o", &m4a]);
    let shown = ok(&["probe", &m4a]);
    assert!(
        shown.contains("ARTIST: Duo") && shown.contains("TITLE: Tone"),
        "{shown}"
    );
    ok(&["convert", &m4a, "-o", &flac]);
    let shown = ok(&["probe", &flac]);
    assert!(
        shown.contains("ARTIST: Duo") && shown.contains("TITLE: Tone"),
        "{shown}"
    );
}

#[test]
fn processing_into_m4a_gives_what_it_gives_into_flac() {
    let dir = Dir::new("processing");
    let wav_in = dir.path("in.wav");
    std::fs::write(&wav_in, mp4_signals::signal("stereo24").wav()).unwrap();
    for extra in [
        vec!["--gain", "-6dB"],
        vec!["--sample-rate", "44100"],
        vec!["--sample-format", "s16"],
        vec!["--sample-format", "s16", "--dither", "none"],
        vec!["--channels", "mono"],
        vec!["--filter", "fade in=0.01s", "--start", "0.005s"],
        vec!["--threads", "1"],
        vec!["--threads", "4"],
    ] {
        let (m4a, flac, from_m4a, from_flac) = (
            dir.path("o.m4a"),
            dir.path("o.flac"),
            dir.path("a.wav"),
            dir.path("b.wav"),
        );
        for out in [&m4a, &flac] {
            let mut args = vec!["convert", "--force", wav_in.as_str(), "-o", out.as_str()];
            args.extend(&extra);
            ok(&args);
        }
        ok(&["convert", "--force", &m4a, "-o", &from_m4a]);
        ok(&["convert", "--force", &flac, "-o", &from_flac]);
        assert!(read(&from_m4a) == read(&from_flac), "{extra:?}");
    }
}

#[test]
fn a_stream_copy_between_m4a_files_keeps_the_packets() {
    let dir = Dir::new("copy");
    for file in [
        "alac-stereo16.m4a",
        "alac-stereo20.m4a",
        "alac-ch6.m4a",
        "alac-stereo32.m4a",
    ] {
        let input = fixture(file);
        let (copy, a, b) = (dir.path("c.m4a"), dir.path("a.wav"), dir.path("b.wav"));
        ok(&["convert", "--force", &input, "--codec", "copy", "-o", &copy]);
        assert!(mdat(&read(&copy)) == mdat(&read(&input)), "{file}");
        ok(&["convert", "--force", &input, "-o", &a]);
        ok(&["convert", "--force", &copy, "-o", &b]);
        assert!(read(&a) == read(&b), "{file}");
    }
}

/// An Ogg Opus file with a pre-skip and a trimmed end.
fn opus() -> Vec<u8> {
    let mut audio = w::packets(9, true, 10, 50, 1);
    audio.extend(w::packets(15, true, 10, 70, 2));
    audio.extend(w::packets(31, true, 10, 90, 3));
    w::ogg_file(
        &w::opus_head(2, 312, 0, None),
        &w::opus_tags("crossfade test", &["TITLE=Tone"]),
        &audio,
        &OggLayout {
            end_trim: 200,
            ..OggLayout::default()
        },
    )
}

#[test]
fn an_opus_stream_copies_into_opus_ogg_and_oga_and_decodes_the_same() {
    let dir = Dir::new("opus");
    let input = dir.path("in.opus");
    std::fs::write(&input, opus()).unwrap();
    let want = dir.path("want.wav");
    ok(&["convert", &input, "--sample-format", "f32", "-o", &want]);
    for name in ["c.opus", "c.ogg", "c.oga"] {
        let (copy, got) = (dir.path(name), dir.path("got.wav"));
        ok(&["convert", "--force", &input, "--codec", "copy", "-o", &copy]);
        assert!(
            ok(&["probe", &copy]).contains("opus, 48000 Hz, stereo"),
            "{name}"
        );
        assert!(ok(&["probe", &copy]).contains("TITLE: Tone"), "{name}");
        ok(&[
            "convert",
            "--force",
            &copy,
            "--sample-format",
            "f32",
            "-o",
            &got,
        ]);
        assert!(wav_data(&read(&got)) == wav_data(&read(&want)), "{name}");
    }
}

#[test]
fn what_alac_cannot_take_is_refused_in_plain_words() {
    let dir = Dir::new("refusals");
    let (stereo, quad) = (dir.path("s.wav"), dir.path("q.wav"));
    std::fs::write(&stereo, mp4_signals::signal("stereo16").wav()).unwrap();
    std::fs::write(&quad, mp4_signals::signal("ch4").wav()).unwrap();
    let o = dir.path("o.m4a");

    let err = refused(&["convert", &stereo, "--sample-format", "f32", "-o", &o]);
    assert!(
        err.contains("alac cannot store floating-point samples"),
        "{err}"
    );
    assert!(err.contains("--sample-format s16, s24 or s32"), "{err}");
    let err = refused(&["convert", &stereo, "--sample-format", "u8", "-o", &o]);
    assert!(err.contains("alac cannot code 8-bit samples"), "{err}");
    let err = refused(&["convert", &stereo, "--compression-level", "5", "-o", &o]);
    assert!(
        err.contains("--compression-level applies to flac only"),
        "{err}"
    );
    let err = refused(&["convert", &stereo, "--codec", "copy", "-o", &o]);
    assert!(
        err.contains("cannot copy pcm_s16le into mp4 (mp4 holds alac only)"),
        "{err}"
    );
    ok(&["convert", &stereo, "--codec", "alac", "-o", &o]);
    let err = refused(&[
        "convert",
        &stereo,
        "--codec",
        "alac",
        "-o",
        &dir.path("o.flac"),
    ]);
    assert!(
        err.contains("codec alac cannot be written into flac"),
        "{err}"
    );
    // Not written: the codec names clap accepts stop at the ones that are.
    for codec in ["aac", "mp3", "opus"] {
        let out = run(&["convert", &stereo, "--codec", codec, "-o", &o]);
        assert_eq!(out.status.code(), Some(2), "{codec}");
        assert!(text(&out.stderr).contains("possible values"), "{codec}");
    }

    // Quad is not ALAC's 4.0 and no standard mix reaches it, so the message says what does.
    let quad_wav = {
        let mut b = mp4_signals::signal("ch4").wav();
        // The channel mask sits 40 bytes into the file in this writer's extensible header.
        let at = b.windows(4).position(|w| w == b"fmt ").unwrap() + 8 + 20;
        b[at..at + 4].copy_from_slice(&0x33u32.to_le_bytes());
        b
    };
    std::fs::write(&quad, quad_wav).unwrap();
    let q = dir.path("q.m4a");
    let err = refused(&["convert", &quad, "-o", &q]);
    assert!(
        err.contains("alac stores 4 channels in one layout only (4.0)"),
        "{err}"
    );
    assert!(err.contains("this stream is quad"), "{err}");
    assert!(err.contains("no standard mix reaches it"), "{err}");
    assert!(!Path::new(&q).exists(), "nothing is created");
}

#[test]
fn an_opus_name_takes_a_stream_copy_only() {
    let dir = Dir::new("opus-name");
    let (wav_in, o) = (dir.path("in.wav"), dir.path("o.opus"));
    std::fs::write(&wav_in, mp4_signals::signal("stereo16").wav()).unwrap();
    let err = refused(&["convert", &wav_in, "-o", &o]);
    assert!(err.contains("encoding opus is not supported yet"), "{err}");
    assert!(err.contains("--codec copy"), "{err}");
    assert!(!Path::new(&o).exists());
}

#[test]
fn the_thread_count_never_changes_the_m4a_bytes() {
    let dir = Dir::new("threads");
    let (input, one, many) = (dir.path("in.wav"), dir.path("1.m4a"), dir.path("4.m4a"));
    std::fs::write(&input, mp4_signals::signal("stereo24").wav()).unwrap();
    ok(&["convert", &input, "--threads", "1", "-o", &one]);
    ok(&["convert", &input, "--threads", "4", "-o", &many]);
    assert!(read(&one) == read(&many));
}

#[test]
fn help_lists_m4a_and_alac() {
    let help = ok(&["convert", "--help"]);
    assert!(help.contains(".m4a/.mp4/.m4b"), "{help}");
    assert!(help.contains("alac"), "{help}");
}
