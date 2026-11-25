//! What a file is, through `cf_inspect`, against the app layer's inspector.
#![allow(clippy::undocumented_unsafe_blocks)]

mod common;

use common::{Dir, c, fixture, json_out, last_error};
use crossfade_app::FsIo;
use crossfade_app::inputs::{InputState, probe_path};
use crossfade_app::inspector;
use crossfade_core::Limits;
use crossfade_ffi::*;
use serde_json::{Value, json};

fn inspect(path: &str) -> Value {
    let path = c(path);
    json_out(|out| unsafe { cf_inspect(path.as_ptr(), out) })
}

/// The app layer's own answer, in the shape the C surface promises.
fn expected(path: &str) -> Value {
    match probe_path(path, &mut FsIo::new(), &Limits::default()) {
        InputState::Probed(doc, _) => {
            let sections: Vec<Value> = inspector::sections(&doc)
                .iter()
                .map(|s| {
                    json!({
                        "title": s.title,
                        "rows": s.rows.iter().map(|r| json!({
                            "field": r.field,
                            "id": r.id,
                            "label": r.label,
                            "value": r.value,
                            "raw": r.raw,
                        })).collect::<Vec<_>>(),
                    })
                })
                .collect();
            json!({
                "readable": true,
                "problem": null,
                "summary": inspector::summary(&doc),
                "sections": sections,
            })
        }
        InputState::Failed(problem) => json!({
            "readable": false,
            "problem": problem,
            "summary": null,
            "sections": [],
        }),
    }
}

fn check(path: &str) -> Value {
    let got = inspect(path);
    let want = expected(path);
    for key in ["readable", "problem", "summary", "sections"] {
        assert_eq!(got[key], want[key], "{path}: {key}");
    }
    assert_eq!(got["path"], path);
    let name = std::path::Path::new(path).file_name().unwrap();
    assert_eq!(got["name"], name.to_string_lossy().as_ref());
    got
}

#[test]
fn every_readable_kind_is_described_as_the_inspector_shows_it() {
    for rel in [
        "wav/f32-5.1.wav",
        "flac/stereo-16bit.flac",
        "flac/tagged.flac",
        "aiff/s24.aiff",
        "ogg/stereo16.oga",
        "mp3/cbr128.mp3",
        "mp4/alac-stereo16.m4a",
    ] {
        let got = check(&fixture(rel));
        assert_eq!(got["readable"], true, "{rel}");
        assert!(got["sections"].as_array().unwrap().len() >= 2, "{rel}");
    }
}

#[test]
fn a_generated_wav_reads_with_its_summary() {
    let dir = Dir::new("inspect-wav");
    let path = common::write_wav(&dir, "a.wav", 48_000, 2, 48_000 * 62 + 12_000);
    let got = check(&path);
    assert_eq!(
        got["summary"],
        "WAV \u{b7} 48 kHz \u{b7} stereo \u{b7} 16-bit \u{b7} 1:02.250"
    );
}

#[test]
fn an_unreadable_file_is_described_with_its_problem_not_refused() {
    let dir = Dir::new("inspect-bad");
    let junk = dir.path("junk.wav");
    std::fs::write(&junk, b"not audio at all").unwrap();
    let got = check(&junk);
    assert_eq!(got["readable"], false);
    assert!(got["problem"].as_str().unwrap().len() > 3);

    let missing = check(&dir.path("missing.flac"));
    assert_eq!(missing["readable"], false);
}

#[test]
fn null_and_bad_text_are_errors() {
    let mut out = std::ptr::null_mut();
    assert_eq!(
        unsafe { cf_inspect(std::ptr::null(), &mut out) },
        CF_ERR_NULL_ARGUMENT
    );
    assert!(out.is_null());
    let bad = [0xffu8, 0xfe, 0];
    assert_eq!(
        unsafe { cf_inspect(bad.as_ptr().cast(), &mut out) },
        CF_ERR_INVALID_UTF8
    );
    assert!(last_error().unwrap().contains("UTF-8"));
    let path = c("x.wav");
    assert_eq!(
        unsafe { cf_inspect(path.as_ptr(), std::ptr::null_mut()) },
        CF_ERR_NULL_ARGUMENT
    );
}
