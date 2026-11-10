//! `validate` is what a front end runs before queueing: it must refuse exactly what
//! `Pipeline::open` refuses, with the same words, and touch no file.
mod common;

use common::{job, sine_wav, spec};
use crossfade_core::{ChannelLayout, Error, Limits, SampleFormat};
use crossfade_graph::{
    AudioSettings, CancelToken, Pipeline, probe, run, validate, writable_extensions,
};
use crossfade_testkit::memory::MemoryIo;

fn settings(format: Option<SampleFormat>, codec: Option<&str>, level: Option<u8>) -> AudioSettings {
    AudioSettings {
        sample_format: format,
        codec: codec.map(Into::into),
        compression_level: level,
    }
}

/// One file per input kind the table uses.
fn io() -> MemoryIo {
    let mut io = MemoryIo::new();
    let stereo = ChannelLayout::stereo();
    for (name, rate, format) in [
        ("s16.wav", 8000, SampleFormat::S16),
        ("s24.wav", 8000, SampleFormat::S24),
        ("s32.wav", 8000, SampleFormat::S32),
        ("f32.wav", 8000, SampleFormat::F32),
        ("hi.wav", 192_000, SampleFormat::S16),
    ] {
        io.insert(name, sine_wav(&spec(rate, format, stereo.clone()), 3000).1);
    }
    // The engine reads nine channels; FLAC cannot hold them.
    let nine = ChannelLayout::unpositioned(9).unwrap();
    io.insert(
        "wide.wav",
        sine_wav(&spec(8000, SampleFormat::S16, nine), 3000).1,
    );
    run(
        &job("s16.wav", "s16.flac", None),
        &mut io,
        &Limits::default(),
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap();
    io
}

type Case = (&'static str, &'static str, AudioSettings);

fn table() -> Vec<Case> {
    use SampleFormat::*;
    vec![
        ("s16.wav", "o.wav", settings(None, None, None)),
        ("s16.wav", "o.WAVE", settings(Some(S24), None, None)),
        ("s16.wav", "o.flac", settings(None, None, None)),
        (
            "s16.wav",
            "o.flac",
            settings(Some(S24), Some("flac"), Some(8)),
        ),
        ("s16.wav", "o.aiff", settings(Some(F32), Some("pcm"), None)),
        ("s16.wav", "o.aifc", settings(None, None, None)),
        // Everything FLAC can hold: 32-bit integers and rates far above 96 kHz.
        ("s32.wav", "o.flac", settings(None, None, None)),
        ("hi.wav", "o.flac", settings(None, None, None)),
        ("s16.flac", "o.flac", settings(None, Some("copy"), None)),
        ("s16.flac", "o.wav", settings(None, None, None)),
        ("s16.wav", "o.wav", settings(None, Some("copy"), None)),
        // Refusals.
        ("f32.wav", "o.flac", settings(None, None, None)),
        ("s16.wav", "o.flac", settings(Some(F64), None, None)),
        ("wide.wav", "o.flac", settings(None, None, None)),
        ("s16.wav", "o.aiff", settings(None, Some("copy"), None)),
        ("s16.flac", "o.wav", settings(None, Some("copy"), None)),
        ("s16.wav", "o.wav", settings(None, Some("flac"), None)),
        ("s16.wav", "o.flac", settings(None, Some("pcm"), None)),
        ("s16.wav", "o.wav", settings(None, None, Some(5))),
        ("s16.wav", "o.flac", settings(None, None, Some(9))),
        ("s16.wav", "o.wav", settings(Some(S24), Some("copy"), None)),
        ("s16.flac", "o.flac", settings(None, Some("copy"), Some(3))),
        ("s16.wav", "o.ogg", settings(None, None, None)),
        ("s16.wav", "o.oga", settings(None, Some("copy"), None)),
        ("s16.wav", "o.mp3", settings(None, None, None)),
        ("s16.wav", "o", settings(None, None, None)),
        ("s16.wav", "o.wav", settings(None, Some("mp3"), None)),
    ]
}

#[test]
fn validate_and_open_agree_on_every_case() {
    let mut refused = 0;
    for (input, output, audio) in table() {
        let mut io = io();
        let limits = Limits::default();
        let stream = probe(input, &mut io, &limits).unwrap().streams[0].clone();
        let checked = validate(&stream, output, &audio);
        let j = {
            let mut j = job(input, output, None);
            j.outputs[0].audio = audio.clone();
            j
        };
        let opened = Pipeline::open(&j, &mut io, &limits);
        let case = format!("{input} -> {output} {audio:?}");
        match (checked, opened) {
            (Ok(planned), Ok(pipeline)) => {
                let plan = pipeline.plan();
                assert_eq!(planned.output, plan.output, "{case}");
                assert_eq!(planned.output_format, plan.output_format, "{case}");
                assert_eq!(planned.copy, plan.copy, "{case}");
            }
            (Err(a), Err(b)) => {
                assert_eq!(a, b, "{case}");
                refused += 1;
            }
            (a, b) => panic!("{case}: validate {:?}, open {:?}", a.err(), b.err()),
        }
    }
    assert_eq!(refused, 16, "the table keeps its refusals");
}

#[test]
fn validate_names_the_cause() {
    let mut io = io();
    let limits = Limits::default();
    let s16 = probe("s16.wav", &mut io, &limits).unwrap().streams[0].clone();
    let text = |s: &AudioSettings, out: &str| validate(&s16, out, s).unwrap_err().to_string();
    assert_eq!(
        text(&settings(None, Some("copy"), None), "o.aiff"),
        "unsupported: cannot copy pcm_s16le into aiff"
    );
    assert_eq!(
        text(&settings(None, Some("flac"), None), "o.wav"),
        "unsupported: codec flac cannot be written into wav"
    );
    assert_eq!(
        text(&settings(None, None, Some(5)), "o.wav"),
        "unsupported: --compression-level applies to flac only"
    );
    let ext = text(&settings(None, None, None), "o.mp3");
    for want in [
        "\"mp3\"",
        "o.mp3",
        ".wav",
        ".flac",
        ".aiff/.aif/.aifc",
        ".ogg can be read but not written yet",
    ] {
        assert!(ext.contains(want), "{want} missing from {ext}");
    }
}

#[test]
fn codec_limits_surface_without_a_file() {
    let mut io = io();
    let limits = Limits::default();
    let stream = probe("wide.wav", &mut io, &limits).unwrap().streams[0].clone();
    let e = validate(&stream, "o.flac", &AudioSettings::default()).unwrap_err();
    assert_eq!(
        e.to_string(),
        "unsupported: flac holds at most 8 channels, got 9"
    );
    for input in ["hi.wav", "s32.wav"] {
        let stream = probe(input, &mut io, &limits).unwrap().streams[0].clone();
        validate(&stream, "o.flac", &AudioSettings::default()).unwrap();
    }
}

#[test]
fn writable_extensions_are_the_containers_that_can_be_written() {
    assert_eq!(
        writable_extensions(),
        ["wav", "flac", "aiff", "aif", "aifc"]
    );
    let mut io = io();
    let limits = Limits::default();
    let stream = probe("s16.wav", &mut io, &limits).unwrap().streams[0].clone();
    for ext in writable_extensions() {
        let out = format!("o.{ext}");
        assert!(
            validate(&stream, &out, &AudioSettings::default()).is_ok(),
            "{ext}"
        );
    }
}
