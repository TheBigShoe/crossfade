//! `validate` is what a front end runs before queueing: it must refuse exactly what
//! `Pipeline::open` refuses, with the same words, and touch no file.
mod common;

use common::{job, sine_wav, spec};
use crossfade_core::{ChannelLayout, Limits, SampleFormat};
use crossfade_opus::testing::ogg::{self as w, OggLayout};
use crossfade_graph::{
    AudioSettings, CancelToken, Pipeline, probe, run, validate, writable_extensions,
};
use crossfade_testkit::memory::MemoryIo;

fn settings(format: Option<SampleFormat>, codec: Option<&str>, level: Option<u8>) -> AudioSettings {
    AudioSettings {
        sample_format: format,
        codec: codec.map(Into::into),
        compression_level: level,
        ..AudioSettings::default()
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
    run(
        &job("s16.wav", "s16.m4a", None),
        &mut io,
        &Limits::default(),
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap();
    let audio = w::packets(9, true, 10, 50, 1);
    io.insert(
        "in.opus",
        w::ogg_file(
            &w::opus_head(2, 312, 0, None),
            &w::opus_tags("t", &[]),
            &audio,
            &OggLayout::default(),
        ),
    );
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
        ("s16.flac", "o.oga", settings(None, Some("copy"), None)),
        (
            "s16.wav",
            "o.oga",
            settings(Some(S24), Some("flac"), Some(8)),
        ),
        ("s16.flac", "o.wav", settings(None, None, None)),
        ("s16.wav", "o.wav", settings(None, Some("copy"), None)),
        ("s16.wav", "o.m4a", settings(None, None, None)),
        ("s24.wav", "o.mp4", settings(None, Some("alac"), None)),
        ("s32.wav", "o.M4B", settings(None, None, None)),
        ("s16.wav", "o.m4a", settings(Some(S24), None, None)),
        ("s16.m4a", "o.m4a", settings(None, Some("copy"), None)),
        ("s16.m4a", "o.flac", settings(None, None, None)),
        ("in.opus", "o.opus", settings(None, Some("copy"), None)),
        ("in.opus", "o.ogg", settings(None, Some("copy"), None)),
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
        ("s16.wav", "o.oga", settings(None, Some("pcm"), None)),
        ("f32.wav", "o.oga", settings(None, None, None)),
        ("s16.wav", "o.mp3", settings(None, None, None)),
        ("s16.wav", "o", settings(None, None, None)),
        ("s16.wav", "o.wav", settings(None, Some("mp3"), None)),
        ("s16.wav", "o.m4a", settings(None, Some("aac"), None)),
        ("s16.wav", "o.m4a", settings(None, Some("flac"), None)),
        ("f32.wav", "o.m4a", settings(None, None, None)),
        ("s16.wav", "o.m4a", settings(Some(U8), None, None)),
        ("s16.wav", "o.m4a", settings(None, None, Some(5))),
        ("s16.wav", "o.m4a", settings(None, Some("copy"), None)),
        ("s16.flac", "o.m4a", settings(None, Some("copy"), None)),
        ("wide.wav", "o.m4a", settings(None, None, None)),
        ("in.opus", "o.opus", settings(None, None, None)),
        ("in.opus", "o.ogg", settings(None, Some("opus"), None)),
        ("s16.m4a", "o.wav", settings(None, Some("copy"), None)),
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
    assert_eq!(refused, 28, "the table keeps its refusals");
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
        text(&settings(None, Some("copy"), None), "o.oga"),
        "unsupported: cannot copy pcm_s16le into ogg (ogg holds flac and opus)"
    );
    assert_eq!(
        text(&settings(None, Some("pcm"), None), "o.oga"),
        "unsupported: codec pcm cannot be written into ogg (ogg writes flac only; an opus \
         stream can be copied with --codec copy)"
    );
    assert_eq!(
        text(&settings(None, Some("copy"), None), "o.m4a"),
        "unsupported: cannot copy pcm_s16le into mp4 (mp4 holds alac only)"
    );
    assert_eq!(
        text(&settings(None, Some("aac"), None), "o.m4a"),
        "unsupported: codec aac cannot be written into mp4 (mp4 writes alac only; aac and mp3 \
         are not written)"
    );
    assert_eq!(
        text(&settings(None, None, Some(5)), "o.m4a"),
        "unsupported: --compression-level applies to flac only"
    );
    let opus = probe("in.opus", &mut io, &limits).unwrap().streams[0].clone();
    assert_eq!(
        validate(&opus, "o.opus", &settings(None, None, None))
            .unwrap_err()
            .to_string(),
        "unsupported: encoding opus is not supported yet; an opus stream can be copied with \
         --codec copy into .opus"
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
        ".oga/.ogg",
        ".m4a/.mp4/.m4b",
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
        ["wav", "flac", "aiff", "aif", "aifc", "oga", "ogg", "m4a", "mp4", "m4b"]
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
