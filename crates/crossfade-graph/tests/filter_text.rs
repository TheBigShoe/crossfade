//! The filter text form: one filter per string, a name, an optional plain value, then
//! `key=value` words, separated by spaces. Units are written and parsed strictly; `Display`
//! is the canonical form and parses back to the same value. The job holds the structured
//! form, not the text.

use crossfade_core::ChannelLayout;
use crossfade_graph::filter::{
    Channels, Curve, FilterSpec, Quality, parse_channels, parse_db, parse_dbtp, parse_lufs,
    parse_rate, parse_time,
};
use crossfade_graph::{AudioSettings, Dither, Job};
use proptest::prelude::*;

fn parse(s: &str) -> Result<FilterSpec, String> {
    s.parse::<FilterSpec>().map_err(|e| e.to_string())
}

#[test]
fn every_filter_in_its_plain_form() {
    let cases = [
        ("gain -6dB", FilterSpec::Gain { db: -6.0 }),
        ("gain +3.5dB", FilterSpec::Gain { db: 3.5 }),
        (
            "trim start=10s end=1:30",
            FilterSpec::Trim {
                start_seconds: Some(10.0),
                end_seconds: Some(90.0),
                duration_seconds: None,
            },
        ),
        (
            "trim duration=500ms",
            FilterSpec::Trim {
                start_seconds: None,
                end_seconds: None,
                duration_seconds: Some(0.5),
            },
        ),
        (
            "fade in=2s",
            FilterSpec::Fade {
                in_seconds: Some(2.0),
                out_seconds: None,
                curve: Curve::Linear,
            },
        ),
        (
            "fade out=3s curve=equal-power",
            FilterSpec::Fade {
                in_seconds: None,
                out_seconds: Some(3.0),
                curve: Curve::EqualPower,
            },
        ),
        (
            "normalize -16LUFS peak=-1dBTP",
            FilterSpec::Normalize {
                lufs: -16.0,
                true_peak_dbtp: -1.0,
            },
        ),
        (
            "normalize -23LUFS",
            FilterSpec::Normalize {
                lufs: -23.0,
                true_peak_dbtp: -1.0,
            },
        ),
        (
            "resample 48000 quality=best",
            FilterSpec::Resample {
                rate: 48000,
                quality: Quality::Best,
            },
        ),
        (
            "resample 44.1kHz",
            FilterSpec::Resample {
                rate: 44100,
                quality: Quality::Good,
            },
        ),
        (
            "mix stereo",
            FilterSpec::Mix {
                channels: Channels::Layout(ChannelLayout::stereo()),
                matrix: None,
            },
        ),
        (
            "mix 1 matrix=0.5,0.5",
            FilterSpec::Mix {
                channels: Channels::Count(1),
                matrix: Some(vec![vec![0.5, 0.5]]),
            },
        ),
    ];
    for (text, want) in cases {
        assert_eq!(parse(text), Ok(want), "{text}");
    }
}

#[test]
fn display_is_the_canonical_form() {
    for (text, canonical) in [
        ("gain +3.5dB", "gain 3.5dB"),
        ("gain   -6dB", "gain -6dB"),
        ("trim end=1:30 start=10s", "trim start=10s end=90s"),
        ("trim start=1:02:03.5", "trim start=3723.5s"),
        ("trim duration=250ms", "trim duration=0.25s"),
        ("fade out=3s in=2s curve=linear", "fade in=2s out=3s"),
        ("fade in=1s curve=s-curve", "fade in=1s curve=s-curve"),
        ("normalize -16LUFS peak=-1dBTP", "normalize -16LUFS"),
        (
            "normalize -14LUFS peak=-2dBTP",
            "normalize -14LUFS peak=-2dBTP",
        ),
        ("resample 48000 quality=good", "resample 48000Hz"),
        (
            "resample 96kHz quality=fast",
            "resample 96000Hz quality=fast",
        ),
        ("mix 5.1-back", "mix 5.1-back"),
        ("mix 2 matrix=1,0/0,1", "mix 2 matrix=1,0/0,1"),
        ("mix mono matrix=0.5,-0.5", "mix mono matrix=0.5,-0.5"),
    ] {
        let spec = parse(text).unwrap();
        assert_eq!(spec.to_string(), canonical, "{text}");
        assert_eq!(parse(canonical), Ok(spec), "{canonical}");
    }
}

/// The offending word, quoted, and what was expected.
#[test]
fn errors_name_the_word_and_say_what_was_expected() {
    let cases = [
        ("", "empty filter; expected a name such as \"gain -3dB\""),
        ("   ", "empty filter; expected a name such as \"gain -3dB\""),
        (
            "blur 3",
            "\"blur\": unknown filter; expected gain, trim, fade, normalize, resample or mix",
        ),
        (
            "Gain -3dB",
            "\"Gain\": unknown filter; expected gain, trim, fade, normalize, resample or mix",
        ),
        ("gain", "gain needs a level, such as \"gain -3dB\""),
        ("gain -3", "\"-3\": expected a level in dB, such as -3dB"),
        (
            "gain -3db",
            "\"-3db\": expected a level in dB, such as -3dB",
        ),
        (
            "gain 1e3dB",
            "\"1e3dB\": expected a level in dB, such as -3dB",
        ),
        (
            "gain infdB",
            "\"infdB\": expected a level in dB, such as -3dB",
        ),
        (
            "gain .5dB",
            "\".5dB\": expected a level in dB, such as -3dB",
        ),
        ("gain 200dB", "\"200dB\": gain must be from -120dB to 60dB"),
        (
            "gain -3dB -4dB",
            "\"-4dB\": gain takes one plain value; give the rest as key=value",
        ),
        ("gain db=-3dB", "\"db=-3dB\": gain takes no options"),
        ("trim", "trim needs start=, end= or duration="),
        (
            "trim 10s",
            "\"10s\": trim takes no plain value; expected start=, end= or duration=",
        ),
        (
            "trim begin=10s",
            "\"begin=10s\": trim has no option begin; expected start=, end= or duration=",
        ),
        (
            "trim start=10",
            "\"start=10\": expected a time such as 10s, 500ms or 1:30",
        ),
        (
            "trim start=",
            "\"start=\": expected a time such as 10s, 500ms or 1:30",
        ),
        (
            "trim start=1:60",
            "\"start=1:60\": expected a time such as 10s, 500ms or 1:30",
        ),
        (
            "trim start=1:5",
            "\"start=1:5\": expected a time such as 10s, 500ms or 1:30",
        ),
        ("trim start=-1s", "\"start=-1s\": a time cannot be negative"),
        (
            "trim start=2000000000s",
            "\"start=2000000000s\": a time must be at most 1000000000s",
        ),
        (
            "trim start=1s start=2s",
            "\"start=2s\": start is given twice",
        ),
        (
            "trim start=5s end=2s",
            "\"end=2s\": end must be after start (5s)",
        ),
        (
            "trim start=5s end=5s",
            "\"end=5s\": end must be after start (5s)",
        ),
        (
            "trim end=5s duration=1s",
            "\"duration=1s\": give end or duration, not both",
        ),
        (
            "trim duration=0s",
            "\"duration=0s\": duration must be more than 0s",
        ),
        ("trim =5s", "\"=5s\": expected key=value"),
        ("fade", "fade needs in= or out="),
        ("fade curve=linear", "fade needs in= or out="),
        ("fade in=0s", "\"in=0s\": in must be more than 0s"),
        (
            "fade in=2s curve=log",
            "\"curve=log\": expected a curve: linear, equal-power or s-curve",
        ),
        (
            "normalize",
            "normalize needs a target, such as \"normalize -16LUFS\"",
        ),
        (
            "normalize -16",
            "\"-16\": expected a loudness in LUFS, such as -16LUFS",
        ),
        (
            "normalize -16LUFS peak=-1",
            "\"peak=-1\": expected a true peak in dBTP, such as -1dBTP",
        ),
        (
            "normalize -80LUFS",
            "\"-80LUFS\": the target must be from -70LUFS to 0LUFS",
        ),
        (
            "normalize -16LUFS peak=1dBTP",
            "\"peak=1dBTP\": peak must be from -60dBTP to 0dBTP",
        ),
        (
            "resample",
            "resample needs a rate, such as \"resample 48000\"",
        ),
        (
            "resample 48k",
            "\"48k\": expected a sample rate such as 48000, 48000Hz or 44.1kHz",
        ),
        (
            "resample 44.1Hz",
            "\"44.1Hz\": expected a sample rate such as 48000, 48000Hz or 44.1kHz",
        ),
        (
            "resample 44.1001kHz",
            "\"44.1001kHz\": expected a sample rate such as 48000, 48000Hz or 44.1kHz",
        ),
        (
            "resample 0",
            "\"0\": the rate must be from 1Hz to 1048575Hz",
        ),
        (
            "resample 2MHz",
            "\"2MHz\": expected a sample rate such as 48000, 48000Hz or 44.1kHz",
        ),
        (
            "resample 48000 quality=max",
            "\"quality=max\": expected a quality: fast, good or best",
        ),
        ("mix", "mix needs a layout, such as \"mix stereo\""),
        (
            "mix surround",
            "\"surround\": expected a layout (mono, stereo, 2.1, 3.0, 3.1, 4.0, quad, quad-side, 5.0, 5.0-back, 5.1, 5.1-back, 6.1, 7.1) or a channel count from 1 to 64",
        ),
        (
            "mix 65",
            "\"65\": expected a layout (mono, stereo, 2.1, 3.0, 3.1, 4.0, quad, quad-side, 5.0, 5.0-back, 5.1, 5.1-back, 6.1, 7.1) or a channel count from 1 to 64",
        ),
        (
            "mix stereo matrix=1,0/0",
            "\"matrix=1,0/0\": every row needs 2 coefficients, like the first",
        ),
        (
            "mix stereo matrix=1,x",
            "\"matrix=1,x\": expected rows of numbers such as 0.5,0.5/1,0",
        ),
        (
            "mix mono matrix=1,0/0,1",
            "\"matrix=1,0/0,1\": mono has 1 channel, so the matrix needs 1 row",
        ),
        (
            "mix mono matrix=20,0",
            "\"matrix=20,0\": coefficients must be from -16 to 16",
        ),
    ];
    for (text, want) in cases {
        assert_eq!(parse(text), Err(want.to_owned()), "{text:?}");
    }
}

#[test]
fn value_words_for_the_shortcut_flags() {
    assert_eq!(parse_db("-6dB").unwrap(), -6.0);
    assert_eq!(parse_time("1:30.5").unwrap(), 90.5);
    assert_eq!(parse_time("0:00").unwrap(), 0.0);
    assert_eq!(parse_time("1500ms").unwrap(), 1.5);
    assert_eq!(parse_time("2:03:04").unwrap(), 7384.0);
    assert_eq!(parse_lufs("-14LUFS").unwrap(), -14.0);
    assert_eq!(parse_dbtp("-2dBTP").unwrap(), -2.0);
    assert_eq!(parse_rate("48kHz").unwrap(), 48000);
    assert_eq!(parse_rate("22050Hz").unwrap(), 22050);
    assert_eq!(parse_channels("6").unwrap(), Channels::Count(6));
    assert_eq!(
        parse_channels("5.1").unwrap(),
        Channels::Layout(ChannelLayout::from_name("5.1").unwrap())
    );
    assert_eq!(
        parse_time("10").unwrap_err().to_string(),
        "\"10\": expected a time such as 10s, 500ms or 1:30"
    );
}

#[test]
fn a_count_keeps_the_layout_it_matches_and_otherwise_takes_the_default() {
    let five_one_back = ChannelLayout::from_name("5.1-back").unwrap();
    assert_eq!(Channels::Count(6).resolve(&five_one_back), five_one_back);
    assert_eq!(
        Channels::Count(6).resolve(&ChannelLayout::stereo()),
        ChannelLayout::from_name("5.1").unwrap()
    );
    assert_eq!(
        Channels::Count(12).resolve(&ChannelLayout::stereo()),
        ChannelLayout::unpositioned(12).unwrap()
    );
    let stereo = Channels::Layout(ChannelLayout::stereo());
    assert_eq!(stereo.resolve(&five_one_back), ChannelLayout::stereo());
}

#[test]
fn the_job_holds_structured_filters() {
    let settings = AudioSettings {
        sample_rate: Some(48000),
        channels: Some(parse_channels("stereo").unwrap()),
        dither: Dither::None,
        filters: vec![
            parse("gain -6dB").unwrap(),
            parse("trim start=10s end=1:30").unwrap(),
            parse("fade out=3s curve=equal-power").unwrap(),
            parse("normalize -16LUFS").unwrap(),
            parse("resample 96kHz quality=best").unwrap(),
            parse("mix 1 matrix=0.5,0.5").unwrap(),
        ],
        ..AudioSettings::default()
    };
    let json = serde_json::to_string(&settings).unwrap();
    assert_eq!(
        json,
        concat!(
            r#"{"sample_rate":48000,"channels":"stereo","dither":"none","filters":["#,
            r#"{"filter":"gain","db":-6.0},"#,
            r#"{"filter":"trim","start_seconds":10.0,"end_seconds":90.0},"#,
            r#"{"filter":"fade","out_seconds":3.0,"curve":"equal-power"},"#,
            r#"{"filter":"normalize","lufs":-16.0,"true_peak_dbtp":-1.0},"#,
            r#"{"filter":"resample","rate":96000,"quality":"best"},"#,
            r#"{"filter":"mix","channels":"1","matrix":[[0.5,0.5]]}]}"#
        )
    );
    assert_eq!(
        serde_json::from_str::<AudioSettings>(&json).unwrap(),
        settings
    );
}

#[test]
fn old_jobs_parse_and_new_fields_are_omitted_when_unused() {
    let old = r#"{"version":1,"inputs":[{"path":"a.wav"}],"outputs":[{"path":"b.flac","audio":{"sample_format":"s16"}}]}"#;
    let job = Job::from_json(old).unwrap();
    let audio = &job.outputs[0].audio;
    assert!(audio.filters.is_empty());
    assert_eq!(
        (audio.sample_rate, &audio.channels, audio.dither),
        (None, &None, Dither::Tpdf)
    );
    let back = job.to_json_pretty();
    for absent in ["filters", "sample_rate", "channels", "dither"] {
        assert!(!back.contains(absent), "{back}");
    }
}

#[test]
fn a_job_file_is_checked_like_the_text() {
    let job = |filters: &str| {
        Job::from_json(&format!(
            r#"{{"version":1,"inputs":[{{"path":"a.wav"}}],"outputs":[{{"path":"b.flac","audio":{{"filters":[{filters}]}}}}]}}"#
        ))
        .map_err(|e| e.to_string())
    };
    assert!(job(r#"{"filter":"gain","db":-3.0}"#).is_ok());
    for (filters, says) in [
        (
            r#"{"filter":"gain","db":300.0}"#,
            "gain must be from -120dB to 60dB",
        ),
        (
            r#"{"filter":"gain","db":-3.0,"extra":1}"#,
            "unknown field `extra`",
        ),
        (r#"{"filter":"blur"}"#, "unknown variant `blur`"),
        (
            r#"{"filter":"trim"}"#,
            "trim needs start=, end= or duration=",
        ),
        (
            r#"{"filter":"trim","start_seconds":5.0,"end_seconds":1.0}"#,
            "end must be after start (5s)",
        ),
        (
            r#"{"filter":"mix","channels":"surround"}"#,
            "expected a layout",
        ),
        (
            r#"{"filter":"fade","in_seconds":1.0,"curve":"log"}"#,
            "expected a curve",
        ),
        (
            r#"{"filter":"resample","rate":0}"#,
            "the rate must be from 1Hz to 1048575Hz",
        ),
    ] {
        let err = job(filters).unwrap_err();
        assert!(err.contains(says), "{filters}: {err}");
    }
    let rate = Job::from_json(
        r#"{"version":1,"inputs":[{"path":"a.wav"}],"outputs":[{"path":"b.flac","audio":{"sample_rate":0}}]}"#,
    );
    assert!(
        rate.unwrap_err()
            .to_string()
            .contains("the rate must be from 1Hz")
    );
}

fn seconds() -> impl Strategy<Value = f64> {
    prop_oneof![
        (0u32..100_000).prop_map(|ms| f64::from(ms) / 1000.0),
        0.0f64..1e8,
    ]
}

fn spec() -> impl Strategy<Value = FilterSpec> {
    let curve = prop_oneof![
        Just(Curve::Linear),
        Just(Curve::EqualPower),
        Just(Curve::SCurve)
    ];
    let quality = prop_oneof![
        Just(Quality::Fast),
        Just(Quality::Good),
        Just(Quality::Best)
    ];
    let channels = prop_oneof![
        (1u16..=64).prop_map(Channels::Count),
        proptest::sample::select(ChannelLayout::NAMES.to_vec())
            .prop_map(|n| Channels::Layout(ChannelLayout::from_name(n).unwrap())),
    ];
    prop_oneof![
        (-120.0f64..=60.0).prop_map(|db| FilterSpec::Gain { db }),
        (
            proptest::option::of(seconds()),
            seconds(),
            0u8..3,
            1e-3f64..1e5
        )
            .prop_map(|(start, end, which, len)| {
                let s = start.unwrap_or(0.0);
                FilterSpec::Trim {
                    // With neither end nor duration, start must be there.
                    start_seconds: if which == 2 { Some(s) } else { start },
                    end_seconds: (which == 0).then_some(s + end + len),
                    duration_seconds: (which == 1).then_some(len),
                }
            }),
        (
            proptest::option::of(1e-3f64..1e4),
            proptest::option::of(1e-3f64..1e4),
            curve
        )
            .prop_filter("a fade needs an end", |(i, o, _)| i.is_some()
                || o.is_some())
            .prop_map(|(i, o, curve)| FilterSpec::Fade {
                in_seconds: i,
                out_seconds: o,
                curve,
            }),
        (-70.0f64..=0.0, -60.0f64..=0.0).prop_map(|(lufs, true_peak_dbtp)| {
            FilterSpec::Normalize {
                lufs,
                true_peak_dbtp,
            }
        }),
        (1u32..=1_048_575, quality)
            .prop_map(|(rate, quality)| FilterSpec::Resample { rate, quality }),
        (
            channels,
            any::<bool>(),
            proptest::collection::vec(-16.0f64..=16.0, 1..8)
        )
            .prop_map(|(channels, with_matrix, coefs)| {
                let rows = match &channels {
                    Channels::Count(n) => usize::from(*n),
                    Channels::Layout(l) => usize::from(l.channels()),
                };
                let matrix = with_matrix.then(|| vec![coefs.clone(); rows]);
                FilterSpec::Mix { channels, matrix }
            }),
    ]
}

proptest! {
    #[test]
    fn display_parses_back_to_the_same_filter(spec in spec()) {
        let text = spec.to_string();
        prop_assert_eq!(text.parse::<FilterSpec>().map_err(|e| e.to_string()), Ok(spec.clone()), "{}", text);
        let json = serde_json::to_string(&spec).unwrap();
        prop_assert_eq!(serde_json::from_str::<FilterSpec>(&json).unwrap(), spec);
    }

    #[test]
    fn any_text_parses_or_fails_without_panicking(text in "\\PC{0,40}") {
        if let Ok(spec) = text.parse::<FilterSpec>() {
            prop_assert_eq!(spec.to_string().parse::<FilterSpec>().ok(), Some(spec));
        }
    }

    #[test]
    fn near_misses_of_real_filters_never_panic(
        name in proptest::sample::select(vec!["gain", "trim", "fade", "normalize", "resample", "mix"]),
        words in proptest::collection::vec("[-+0-9.:=a-zA-Z,/]{0,12}", 0..4),
    ) {
        let text = format!("{name} {}", words.join(" "));
        if let Ok(spec) = text.parse::<FilterSpec>() {
            prop_assert_eq!(spec.to_string().parse::<FilterSpec>().ok(), Some(spec));
        }
    }
}
