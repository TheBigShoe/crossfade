//! Processing on the command line: `--filter`, the shortcut flags and their fixed order,
//! output rate and channels, dither, `concat`, and the clip warning.

use crossfade_cli::{clip_warning, request_from_args};
use crossfade_graph::Job;
use std::path::PathBuf;
use std::process::{Command, Output};

fn job<S: AsRef<str>>(args: &[S]) -> Job {
    request_from_args(args.iter().map(AsRef::as_ref))
        .unwrap()
        .unwrap()
        .job
}

fn filter_texts<S: AsRef<str>>(args: &[S]) -> Vec<String> {
    job(args).outputs[0]
        .audio
        .filters
        .iter()
        .map(ToString::to_string)
        .collect()
}

fn error<S: AsRef<str>>(args: &[S]) -> String {
    request_from_args(args.iter().map(AsRef::as_ref))
        .unwrap_err()
        .to_string()
}

const CONVERT: [&str; 4] = ["convert", "in.wav", "-o", "out.flac"];

fn with(extra: &[&str]) -> Vec<String> {
    CONVERT
        .iter()
        .chain(extra)
        .map(|s| (*s).to_owned())
        .collect()
}

#[test]
fn a_filter_and_its_shortcut_give_the_same_job() {
    for (flag, filter) in [
        (&["--gain", "-6dB"][..], "gain -6dB"),
        (
            &["--start", "10s", "--end", "1:30"],
            "trim start=10s end=1:30",
        ),
        (&["--duration", "500ms"], "trim duration=500ms"),
        (
            &["--fade-in", "2s", "--fade-out", "3s"],
            "fade in=2s out=3s",
        ),
        (&["--normalize", "-16LUFS"], "normalize -16LUFS"),
        (
            &["--normalize", "-16LUFS", "--true-peak", "-2dBTP"],
            "normalize -16LUFS peak=-2dBTP",
        ),
    ] {
        assert_eq!(
            job(&with(flag)),
            job(&with(&["--filter", filter])),
            "{flag:?}"
        );
    }
}

/// The documented order: the trim shortcuts, every `--filter` as given, then the fades, the
/// gain and the normalisation, whatever order the flags were typed in.
#[test]
fn shortcuts_take_their_fixed_places_around_the_filters() {
    let typed = with(&[
        "--normalize",
        "-14LUFS",
        "--fade-out",
        "4s",
        "--filter",
        "mix stereo",
        "--start",
        "30s",
        "--filter",
        "resample 48kHz",
        "--fade-in",
        "1s",
        "--duration",
        "2:00",
    ]);
    insta::assert_snapshot!(filter_texts(&typed).join("\n"), @r"
    trim start=30s duration=120s
    mix stereo
    resample 48000Hz
    fade in=1s out=4s
    normalize -14LUFS
    ");
    assert_eq!(
        filter_texts(&with(&["--gain", "-1dB", "--fade-in", "1s", "--end", "9s"])),
        ["trim end=9s", "fade in=1s", "gain -1dB"]
    );
}

#[test]
fn the_job_holds_the_filter_list_and_the_settings() {
    let json = job(&with(&[
        "--gain",
        "-3dB",
        "--sample-rate",
        "44.1kHz",
        "--channels",
        "mono",
        "--dither",
        "none",
    ]))
    .to_json_pretty();
    insta::assert_snapshot!(json, @r#"
    {
      "version": 1,
      "inputs": [
        {
          "path": "in.wav"
        }
      ],
      "outputs": [
        {
          "path": "out.flac",
          "audio": {
            "sample_rate": 44100,
            "channels": "mono",
            "dither": "none",
            "filters": [
              {
                "filter": "gain",
                "db": -3.0
              }
            ]
          }
        }
      ]
    }
    "#);
    // Asking for the default dither is the same job as not asking.
    assert_eq!(job(&with(&["--dither", "tpdf"])), job(&with(&[])));
}

#[test]
fn bad_values_are_refused_with_what_was_expected() {
    for (extra, says) in [
        (
            &["--filter", "gain -3"][..],
            "\"-3\": expected a level in dB, such as -3dB",
        ),
        (&["--filter", "blur"], "\"blur\": unknown filter"),
        (
            &["--gain", "-3"],
            "\"-3\": expected a level in dB, such as -3dB",
        ),
        (
            &["--gain", "200dB"],
            "\"200dB\": gain must be from -120dB to 60dB",
        ),
        (
            &["--start", "10"],
            "\"10\": expected a time such as 10s, 500ms or 1:30",
        ),
        (
            &["--start", "10s", "--end", "5s"],
            "end must be after start (10s)",
        ),
        (&["--fade-in", "0s"], "in must be more than 0s"),
        (&["--normalize", "-16"], "expected a loudness in LUFS"),
        (
            &["--normalize", "-90LUFS"],
            "the target must be from -70LUFS to 0LUFS",
        ),
        (
            &["--sample-rate", "48k"],
            "expected a sample rate such as 48000",
        ),
        (&["--sample-rate", "0"], "the rate must be from 1Hz"),
        (&["--channels", "surround"], "expected a layout"),
        (&["--dither", "rect"], "invalid value 'rect' for '--dither"),
        (&["--end", "5s", "--duration", "1s"], "cannot be used with"),
        (
            &["--gain", "-1dB", "--normalize", "-16LUFS"],
            "cannot be used with",
        ),
        (&["--true-peak", "-1dBTP"], "--normalize <LUFS>"),
    ] {
        let err = error(&with(extra));
        assert!(err.contains(says), "{extra:?}: {err}");
    }
}

#[test]
fn concat_takes_two_or_more_inputs_and_keeps_their_order() {
    let j = job(&[
        "concat", "b.wav", "a.wav", "c.flac", "-o", "all.flac", "--gain", "-1dB",
    ]);
    let paths: Vec<&str> = j.inputs.iter().map(|i| i.path.as_str()).collect();
    assert_eq!(paths, ["b.wav", "a.wav", "c.flac"]);
    assert_eq!(j.outputs[0].path, "all.flac");
    assert_eq!(j.outputs[0].audio.filters.len(), 1);
    let one = error(&["concat", "a.wav", "-o", "all.flac"]);
    assert!(one.contains("<INPUTS> <INPUTS>..."), "{one}");
    // convert never joins: a second input is an error, so a glob cannot join files.
    assert!(request_from_args(["convert", "a.wav", "b.wav", "-o", "c.flac"]).is_err());
}

#[test]
fn the_clip_warning_counts_and_suggests() {
    assert_eq!(
        clip_warning(1),
        "1 sample clipped: the output format cannot hold it. Lower the level (for example \
         --gain -3dB) or write floating point (--sample-format f32)"
    );
    assert!(
        clip_warning(1234).starts_with("1234 samples clipped: the output format cannot hold them.")
    );
}

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p =
            std::env::temp_dir().join(format!("crossfade-filters-{name}-{}", std::process::id()));
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

/// Mono 16-bit 8 kHz WAV holding `samples`.
fn write_wav(dir: &Dir, name: &str, samples: &[i16]) -> String {
    let data = u32::try_from(samples.len() * 2).unwrap();
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
    for s in samples {
        v.extend_from_slice(&s.to_le_bytes());
    }
    let path = dir.path(name);
    std::fs::write(&path, v).unwrap();
    path
}

fn crossfade(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_crossfade"))
        .args(args)
        .output()
        .unwrap()
}

fn text(b: &[u8]) -> String {
    String::from_utf8(b.to_vec()).unwrap()
}

fn samples_of(path: &str) -> Vec<i16> {
    let bytes = std::fs::read(path).unwrap();
    bytes[44..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&b| i16::from_le_bytes(b))
        .collect()
}

#[test]
fn gain_runs_end_to_end_and_both_spellings_write_the_same_file() {
    let d = Dir::new("gain");
    let x: Vec<i16> = (0..8000)
        .map(|i| ((i * 37) % 20_000 - 10_000) as i16)
        .collect();
    let input = write_wav(&d, "in.wav", &x);
    let (a, b) = (d.path("a.flac"), d.path("b.flac"));
    for (out, flags) in [(&a, ["--gain", "-6dB"]), (&b, ["--filter", "gain -6dB"])] {
        let o = crossfade(&[&["convert", &input, "-o", out][..], &flags[..]].concat());
        assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
        assert!(o.stderr.is_empty(), "{}", text(&o.stderr));
    }
    assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
    // Without dither, -20 dB (exactly 0.1) rounds half away from zero.
    let tenth = d.path("tenth.wav");
    let o = crossfade(&[
        "convert", &input, "-o", &tenth, "--gain", "-20dB", "--dither", "none",
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    let want: Vec<i16> = x
        .iter()
        .map(|&s| (f64::from(s) * 0.1).round() as i16)
        .collect();
    assert_eq!(samples_of(&tenth), want);
}

#[test]
fn clipping_is_a_warning_on_stderr_and_the_file_is_still_written() {
    let d = Dir::new("clip");
    let input = write_wav(&d, "in.wav", &[0, 100, 5000, -5000, 20_000, 3000]);
    let out = d.path("out.wav");
    let o = crossfade(&["convert", &input, "-o", &out, "--gain", "+20dB"]);
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(
        text(&o.stderr),
        format!("crossfade: warning: {}\n", clip_warning(3))
    );
    assert!(std::fs::metadata(&out).is_ok());
}

#[test]
fn stream_copy_with_a_filter_is_refused_with_the_reason() {
    let d = Dir::new("copy");
    let input = write_wav(&d, "in.wav", &[1, 2, 3]);
    let o = crossfade(&[
        "convert",
        &input,
        "-o",
        &d.path("out.wav"),
        "--codec",
        "copy",
        "--gain",
        "-1dB",
    ]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        text(&o.stderr).contains("--codec copy cannot apply filters (copying passes"),
        "{}",
        text(&o.stderr)
    );
}

#[test]
fn concat_joins_files_end_to_end() {
    let d = Dir::new("concat");
    let a = write_wav(&d, "a.wav", &[1, 2, 3]);
    let b = write_wav(&d, "b.wav", &[4, 5]);
    let c = write_wav(&d, "c.wav", &[-6]);
    let out = d.path("abc.wav");
    let o = crossfade(&["concat", &a, &b, &c, "-o", &out]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    assert!(o.stderr.is_empty(), "{}", text(&o.stderr));
    assert_eq!(samples_of(&out), [1, 2, 3, 4, 5, -6]);
    // Joined with a gain on the whole: one filter, both inputs.
    let loud = d.path("loud.wav");
    let o = crossfade(&[
        "concat", &a, &b, "-o", &loud, "--gain", "+20dB", "--dither", "none",
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    assert_eq!(samples_of(&loud), [10, 20, 30, 40, 50]);
    let o = crossfade(&["concat", &a, &b, "-o", &out, "--emit-job"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(text(&o.stdout).contains("b.wav"));
    // A missing input fails before anything is written.
    let missing = d.path("missing.wav");
    let o = crossfade(&["concat", &a, &missing, "-o", &d.path("x.wav")]);
    assert_eq!(o.status.code(), Some(1));
    assert!(std::fs::metadata(d.path("x.wav")).is_err());
}

#[test]
fn concat_conforms_a_second_rate_to_the_first() {
    let d = Dir::new("concat-rate");
    let a = write_wav(&d, "a.wav", &[100; 8000]);
    // The same, but at 16 kHz.
    let mut b16 = std::fs::read(write_wav(&d, "tmp.wav", &[100; 16_000])).unwrap();
    b16[24..28].copy_from_slice(&16_000u32.to_le_bytes());
    b16[28..32].copy_from_slice(&32_000u32.to_le_bytes());
    let b = d.path("b.wav");
    std::fs::write(&b, b16).unwrap();
    let out = d.path("ab.flac");
    let o = crossfade(&["concat", &a, &b, "-o", &out]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    let probe = text(&crossfade(&["probe", &out, "--json"]).stdout);
    assert!(probe.contains("\"sample_rate\": 8000"), "{probe}");
    assert!(probe.contains("\"frames\": 16000"), "{probe}");
}
