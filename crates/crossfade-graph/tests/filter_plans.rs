//! "Filter strings parse to job plans": a table of inputs and settings, and for each the job
//! data, the chain the engine would run, the output it describes and what `validate` says.
//! Read the snapshot (`snapshots/filter_plans__plans.snap`) as the specification.

use crossfade_core::{AudioSpec, ChannelLayout, CodecId, SampleFormat, StreamInfo, TimeBase};
use crossfade_graph::filter::{FilterSpec, parse_channels, parse_rate};
use crossfade_graph::{
    AudioSettings, Dither, describe, describe_concat, filter, validate, validate_concat,
};
use std::fmt::Write;

fn stream(rate: u32, format: SampleFormat, layout: &str, frames: Option<u64>) -> StreamInfo {
    let layout = ChannelLayout::from_name(layout)
        .unwrap_or_else(|| ChannelLayout::unpositioned(layout.parse().unwrap()).unwrap());
    let codec = if format.is_float() {
        CodecId::pcm_for(format)
    } else {
        CodecId::Flac
    };
    StreamInfo {
        frames,
        ..StreamInfo::new(
            0,
            codec,
            AudioSpec::new(rate, format, layout).unwrap(),
            TimeBase::new(1, i64::from(rate)).unwrap(),
        )
    }
}

fn cd(frames: u64) -> StreamInfo {
    stream(44100, SampleFormat::S16, "stereo", Some(frames))
}

fn describe_stream(s: &StreamInfo) -> String {
    let layout = s.audio.layout().name().map_or_else(
        || format!("{} channels", s.audio.layout().channels()),
        str::to_owned,
    );
    let depth = s
        .bits_per_raw_sample
        .map_or_else(|| "full width".to_owned(), |b| format!("{b}-bit"));
    let frames = s
        .frames
        .map_or_else(|| "length unknown".to_owned(), |f| format!("{f} frames"));
    format!(
        "{} Hz {layout} {} ({depth}), {frames}",
        s.audio.sample_rate(),
        s.audio.format().name()
    )
}

/// CLI-like words for the settings, applied in order.
fn settings(words: &[&str]) -> Result<AudioSettings, String> {
    let mut s = AudioSettings::default();
    for w in words {
        let (k, v) = w.split_once(' ').unwrap_or((w, ""));
        match k {
            "--filter" => s
                .filters
                .push(v.parse::<FilterSpec>().map_err(|e| e.to_string())?),
            "--sample-rate" => s.sample_rate = Some(parse_rate(v).map_err(|e| e.to_string())?),
            "--channels" => s.channels = Some(parse_channels(v).map_err(|e| e.to_string())?),
            "--sample-format" => s.sample_format = SampleFormat::from_name(v),
            "--codec" => s.codec = Some(v.into()),
            "--dither" => s.dither = Dither::from_name(v).unwrap(),
            other => panic!("{other}"),
        }
    }
    Ok(s)
}

struct Case {
    title: &'static str,
    inputs: Vec<StreamInfo>,
    output: &'static str,
    words: &'static [&'static str],
}

fn case(
    title: &'static str,
    input: StreamInfo,
    output: &'static str,
    words: &'static [&'static str],
) -> Case {
    Case {
        title,
        inputs: vec![input],
        output,
        words,
    }
}

fn render(c: &Case) -> String {
    let mut out = format!("### {}\n", c.title);
    for (i, input) in c.inputs.iter().enumerate() {
        let label = if c.inputs.len() == 1 {
            "input:   ".to_owned()
        } else {
            format!("input {}: ", i + 1)
        };
        writeln!(out, "{label}{}", describe_stream(input)).unwrap();
    }
    let args: Vec<String> = c
        .words
        .iter()
        .map(|w| match w.split_once(' ') {
            Some((k, v)) if v.contains(' ') => format!("{k} \"{v}\""),
            _ => (*w).to_owned(),
        })
        .collect();
    writeln!(out, "args:     -o {} {}", c.output, args.join(" ")).unwrap();
    let s = match settings(c.words) {
        Ok(s) => s,
        Err(e) => {
            writeln!(out, "error:    {e}\n").unwrap();
            return out;
        }
    };
    writeln!(out, "job:      {}", serde_json::to_string(&s).unwrap()).unwrap();
    if c.inputs.len() == 1 {
        match filter::plan(&c.inputs[0].audio, &s) {
            Ok(None) => writeln!(out, "chain:    none (the bit-exact path)").unwrap(),
            Ok(Some(p)) => {
                for (i, f) in p.filters.iter().enumerate() {
                    let label = if i == 0 { "chain:   " } else { "         " };
                    writeln!(out, "{label} {}. {f}", i + 1).unwrap();
                }
                writeln!(out, "dither:   {}", p.dither.name()).unwrap();
            }
            Err(e) => writeln!(out, "chain:    error: {e}").unwrap(),
        }
    }
    let (described, validated) = if c.inputs.len() == 1 {
        (
            describe(&c.inputs[0], c.output, &s),
            validate(&c.inputs[0], c.output, &s),
        )
    } else {
        (
            describe_concat(&c.inputs, c.output, &s),
            validate_concat(&c.inputs, c.output, &s),
        )
    };
    match described {
        Ok(p) => {
            let copy = if p.copy { " (stream copy)" } else { "" };
            writeln!(
                out,
                "output:   {} {}{copy}",
                describe_stream(&p.output),
                p.output.codec.name()
            )
            .unwrap();
        }
        Err(e) => writeln!(out, "output:   error: {e}").unwrap(),
    }
    match validated {
        Ok(_) => writeln!(out, "validate: ok").unwrap(),
        Err(e) => writeln!(out, "validate: {e}").unwrap(),
    }
    out.push('\n');
    out
}

#[test]
fn plans() {
    let twenty = StreamInfo {
        bits_per_raw_sample: Some(20),
        ..stream(96000, SampleFormat::S24, "stereo", Some(960_000))
    };
    let cases = vec![
        // Nothing asked: today's path, untouched.
        case("no filters", cd(441_000), "o.flac", &[]),
        case(
            "the rate and layout the input already has",
            cd(441_000),
            "o.flac",
            &["--sample-rate 44100", "--channels stereo", "--channels 2"],
        ),
        case(
            "a 20-bit source keeps its depth without filters",
            twenty.clone(),
            "o.flac",
            &[],
        ),
        // Gain: built.
        case("gain", cd(441_000), "o.flac", &["--filter gain -6dB"]),
        case(
            "gain without dither",
            cd(441_000),
            "o.wav",
            &["--filter gain +3.5dB", "--dither none"],
        ),
        case(
            "gain on a 20-bit source writes full width",
            twenty.clone(),
            "o.flac",
            &["--filter gain -1dB"],
        ),
        case(
            "gain into float",
            cd(441_000),
            "o.wav",
            &["--filter gain -1dB", "--sample-format f32"],
        ),
        case(
            "two gains run in order",
            cd(441_000),
            "o.flac",
            &["--filter gain -3dB", "--filter gain +1dB"],
        ),
        case(
            "an unknown length stays unknown",
            stream(48000, SampleFormat::S24, "mono", None),
            "o.flac",
            &["--filter gain -3dB"],
        ),
        // Trim.
        case(
            "trim start and end",
            cd(441_000 * 6),
            "o.flac",
            &["--filter trim start=10s end=1:00"],
        ),
        case(
            "trim by duration in milliseconds",
            cd(441_000),
            "o.flac",
            &["--filter trim start=1500ms duration=250ms"],
        ),
        case(
            "trim past the end leaves nothing",
            cd(44100),
            "o.flac",
            &["--filter trim start=2s"],
        ),
        case(
            "trim end on an input of unknown length",
            stream(48000, SampleFormat::S16, "stereo", None),
            "o.flac",
            &["--filter trim end=0:30"],
        ),
        case(
            "trim to a sample-accurate frame",
            stream(48000, SampleFormat::S16, "stereo", Some(480_000)),
            "o.wav",
            &["--filter trim start=0.0000208s end=1:00:00"],
        ),
        // Fade.
        case(
            "fade in and out",
            cd(441_000),
            "o.flac",
            &["--filter fade in=2s out=3s curve=equal-power"],
        ),
        case(
            "fade after trim counts at the trimmed stream",
            cd(441_000),
            "o.flac",
            &[
                "--filter trim start=5s",
                "--filter fade out=500ms curve=s-curve",
            ],
        ),
        // Normalize.
        case(
            "normalize",
            cd(441_000),
            "o.flac",
            &["--filter normalize -16LUFS"],
        ),
        case(
            "normalize with a ceiling",
            cd(441_000),
            "o.flac",
            &["--filter normalize -23LUFS peak=-2dBTP"],
        ),
        case(
            "two normalize filters",
            cd(441_000),
            "o.flac",
            &["--filter normalize -16LUFS", "--filter normalize -14LUFS"],
        ),
        // Resample: explicit, and asked for as an output setting.
        case(
            "resample filter",
            cd(441_000),
            "o.flac",
            &["--filter resample 48kHz quality=best"],
        ),
        case(
            "output rate",
            cd(441_000),
            "o.flac",
            &["--sample-rate 48000"],
        ),
        case(
            "output rate after a gain",
            cd(441_000),
            "o.flac",
            &["--filter gain -1dB", "--sample-rate 96kHz"],
        ),
        case(
            "an explicit resample then back by the output setting",
            cd(441_000),
            "o.flac",
            &["--filter resample 96000", "--sample-rate 44100"],
        ),
        case(
            "an odd rate count is rounded up",
            cd(1),
            "o.flac",
            &["--sample-rate 48000"],
        ),
        // Mix: explicit, and asked for as an output setting.
        case("mix to mono", cd(441_000), "o.flac", &["--filter mix mono"]),
        case(
            "output channels down: mixed before resampling",
            stream(48000, SampleFormat::S24, "5.1", Some(48000)),
            "o.flac",
            &["--channels stereo", "--sample-rate 44.1kHz"],
        ),
        case(
            "output channels up: resampled before mixing",
            cd(441_000),
            "o.wav",
            &["--channels 5.1", "--sample-rate 48000"],
        ),
        case(
            "a count takes the default layout for it",
            cd(441_000),
            "o.wav",
            &["--channels 6"],
        ),
        case(
            "a matrix",
            cd(441_000),
            "o.flac",
            &["--filter mix mono matrix=0.5,0.5"],
        ),
        case(
            "a matrix that does not fit the input",
            cd(441_000),
            "o.flac",
            &["--filter mix mono matrix=0.3,0.3,0.3"],
        ),
        case(
            "unpositioned input to a named layout",
            stream(48000, SampleFormat::S16, "2", Some(1000)),
            "o.flac",
            &["--channels stereo"],
        ),
        case(
            "more channels than FLAC holds",
            cd(441_000),
            "o.flac",
            &["--channels 12"],
        ),
        // Everything at once.
        case(
            "a full chain",
            stream(96000, SampleFormat::S24, "5.1", Some(96000 * 300)),
            "o.flac",
            &[
                "--filter trim start=30s duration=2:00",
                "--filter mix stereo",
                "--filter fade in=1s out=4s",
                "--filter normalize -14LUFS peak=-1.5dBTP",
                "--sample-rate 48000",
                "--sample-format s16",
            ],
        ),
        // Stream copy: refused when anything would change the audio.
        case(
            "copy with a filter",
            stream(44100, SampleFormat::S16, "stereo", Some(1000)),
            "o.flac",
            &["--codec copy", "--filter gain -1dB"],
        ),
        case(
            "copy with a new rate",
            cd(1000),
            "o.flac",
            &["--codec copy", "--sample-rate 48000"],
        ),
        case(
            "copy with new channels",
            cd(1000),
            "o.flac",
            &["--codec copy", "--channels mono"],
        ),
        case(
            "copy with the same rate and channels",
            cd(1000),
            "o.flac",
            &["--codec copy", "--sample-rate 44100", "--channels stereo"],
        ),
        // Words that do not parse never reach a job.
        case(
            "a level without its unit",
            cd(1000),
            "o.flac",
            &["--filter gain -3"],
        ),
        case(
            "a misspelt filter",
            cd(1000),
            "o.flac",
            &["--filter gian -3dB"],
        ),
        case(
            "a misspelt option",
            cd(1000),
            "o.flac",
            &["--filter fade in=2s curve=exp"],
        ),
        case(
            "a time that is not one",
            cd(1000),
            "o.flac",
            &["--filter trim start=90"],
        ),
        case(
            "a rate with a stray unit",
            cd(1000),
            "o.flac",
            &["--sample-rate 48k"],
        ),
        case(
            "an unknown layout",
            cd(1000),
            "o.flac",
            &["--channels 5.1(side)"],
        ),
        // Concat: the plan exists, the pipeline does not yet.
        Case {
            title: "concat of matching inputs",
            inputs: vec![cd(441_000), cd(220_500)],
            output: "o.flac",
            words: &[],
        },
        Case {
            title: "concat conforms to the first input",
            inputs: vec![
                cd(441_000),
                stream(48000, SampleFormat::S24, "mono", Some(48000)),
            ],
            output: "o.flac",
            words: &["--filter fade out=2s"],
        },
        Case {
            title: "concat conforms to the output settings",
            inputs: vec![
                cd(441_000),
                stream(48000, SampleFormat::S24, "mono", Some(48000)),
            ],
            output: "o.wav",
            words: &["--sample-rate 48000", "--channels mono"],
        },
        Case {
            title: "concat with an input of unknown length",
            inputs: vec![
                cd(441_000),
                stream(44100, SampleFormat::S16, "stereo", None),
            ],
            output: "o.flac",
            words: &[],
        },
        Case {
            title: "concat cannot copy",
            inputs: vec![cd(441_000), cd(441_000)],
            output: "o.flac",
            words: &["--codec copy"],
        },
    ];
    let table: String = cases.iter().map(render).collect();
    insta::assert_snapshot!(table);
}
