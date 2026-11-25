//! The JSON documents Swift and Kotlin decode, pinned: a change to a key, a type or a value's
//! spelling fails here first. Paths, versions, revisions and the meter's figures are replaced
//! by placeholders.
#![allow(clippy::undocumented_unsafe_blocks)]

mod common;

use common::{Dir, c, fixture, json_out, write_wav};
use crossfade_ffi::*;
use serde_json::{Value, json};

/// The document with the scratch directory, the fixtures' directory, the version and any
/// revision replaced, pretty-printed.
fn shown(mut v: Value, dir: &str) -> String {
    let fixtures = fixture("");
    fn walk(v: &mut Value, swaps: &[(&str, &str)]) {
        match v {
            Value::String(s) => {
                for (from, to) in swaps {
                    *s = s.replace(from, to);
                }
            }
            Value::Array(a) => a.iter_mut().for_each(|x| walk(x, swaps)),
            Value::Object(o) => {
                for (k, x) in o.iter_mut() {
                    if k == "revision" {
                        *x = json!("<revision>");
                    } else if k == "version" && x.is_string() {
                        *x = json!("<version>");
                    } else if ["integrated", "range", "true_peak", "sample_peak"]
                        .contains(&k.as_str())
                        && x.is_number()
                    {
                        // The meter's figures are its own tests' business; the type is pinned.
                        *x = json!("<number>");
                    } else {
                        walk(x, swaps);
                    }
                }
            }
            _ => {}
        }
    }
    walk(
        &mut v,
        &[
            (fixtures.trim_end_matches('/'), "<fixtures>"),
            (dir, "<dir>"),
            (env!("CARGO_PKG_VERSION"), "<version>"),
        ],
    );
    serde_json::to_string_pretty(&sorted(v)).unwrap()
}

/// The document with every object's keys in order. A whole-workspace build turns on
/// serde_json's `preserve_order` (GPUI asks for it), which would otherwise print the keys in
/// the order they were inserted and make these snapshots depend on what else was built.
fn sorted(v: Value) -> Value {
    match v {
        Value::Array(a) => Value::Array(a.into_iter().map(sorted).collect()),
        Value::Object(o) => {
            let mut entries: Vec<(String, Value)> = o.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(entries.into_iter().map(|(k, x)| (k, sorted(x))).collect())
        }
        other => other,
    }
}

fn inspect(path: &str) -> Value {
    let p = c(path);
    json_out(|out| unsafe { cf_inspect(p.as_ptr(), out) })
}

#[test]
fn about_shape() {
    let v = json_out(|out| unsafe { cf_about(out) });
    insta::assert_snapshot!(shown(v, "/nowhere"));
}

#[test]
fn inspect_shapes() {
    let dir = Dir::new("shapes-inspect");
    let root = dir.0.to_string_lossy().into_owned();
    insta::assert_snapshot!(
        "inspect_readable",
        shown(inspect(&fixture("flac/tagged.flac")), &root)
    );
    let junk = dir.path("junk.wav");
    std::fs::write(&junk, b"nope").unwrap();
    insta::assert_snapshot!("inspect_unreadable", shown(inspect(&junk), &root));
}

fn sheet_state(path: &str, changes: Value) -> Value {
    let p = c(path);
    let mut sheet = std::ptr::null_mut();
    assert_eq!(unsafe { cf_sheet_new(p.as_ptr(), &mut sheet) }, CF_OK);
    let changes = c(&changes.to_string());
    assert_eq!(unsafe { cf_sheet_apply(sheet, changes.as_ptr()) }, CF_OK);
    let state = json_out(|out| unsafe { cf_sheet_state(sheet, out) });
    unsafe { cf_sheet_destroy(sheet) };
    state
}

#[test]
fn sheet_shapes() {
    let dir = Dir::new("shapes-sheet");
    let root = dir.0.to_string_lossy().into_owned();
    let input = write_wav(&dir, "in.wav", 44_100, 2, 44_100);
    insta::assert_snapshot!(
        "sheet_default",
        shown(sheet_state(&input, json!({})), &root)
    );
    let opus = json!([{"container": "opus"}, {"bitrate": 96000}, {"adjust_open": true},
        {"level_mode": "normalize"}, {"fade_out": "0.5"}]);
    insta::assert_snapshot!("sheet_opus", shown(sheet_state(&input, opus), &root));
    let problems = json!([{"output": dir.path("x.mp3")}]);
    insta::assert_snapshot!(
        "sheet_problems",
        shown(sheet_state(&input, problems), &root)
    );
}

#[test]
fn queue_shape() {
    let dir = Dir::new("shapes-queue");
    let root = dir.0.to_string_lossy().into_owned();
    let input = write_wav(&dir, "in.wav", 48_000, 2, 4_800);
    let mut q = std::ptr::null_mut();
    assert_eq!(unsafe { cf_queue_new(&mut q) }, CF_OK);
    let p = c(&input);
    let mut sheet = std::ptr::null_mut();
    assert_eq!(unsafe { cf_sheet_new(p.as_ptr(), &mut sheet) }, CF_OK);
    let changes =
        c(&json!({"container": "flac", "level_mode": "gain", "level_value": "-1"}).to_string());
    assert_eq!(unsafe { cf_sheet_apply(sheet, changes.as_ptr()) }, CF_OK);
    let mut id = 0;
    assert_eq!(unsafe { cf_queue_add(q, sheet, &mut id) }, CF_OK);
    unsafe { cf_sheet_destroy(sheet) };
    loop {
        let state = json_out(|out| unsafe { cf_queue_state(q, out) });
        if state["idle"] == true {
            insta::assert_snapshot!(shown(state, &root));
            break;
        }
        let mut now = 0;
        let at = state["revision"].as_u64().unwrap();
        assert_eq!(unsafe { cf_queue_wait(q, at, 60_000, &mut now) }, CF_OK);
    }
    unsafe { cf_queue_destroy(q) };
}

#[test]
fn analysis_shape() {
    let dir = Dir::new("shapes-analysis");
    let root = dir.0.to_string_lossy().into_owned();
    let input = write_wav(&dir, "in.wav", 48_000, 2, 48_000 * 4);
    let p = c(&input);
    let mut a = std::ptr::null_mut();
    assert_eq!(unsafe { cf_analysis_new(p.as_ptr(), &mut a) }, CF_OK);
    loop {
        let state = json_out(|out| unsafe { cf_analysis_state(a, out) });
        if state["state"] != "running" {
            insta::assert_snapshot!(shown(state, &root));
            break;
        }
        let mut now = 0;
        let at = state["revision"].as_u64().unwrap();
        assert_eq!(unsafe { cf_analysis_wait(a, at, 60_000, &mut now) }, CF_OK);
    }
    unsafe { cf_analysis_destroy(a) };
}

#[test]
fn player_shape() {
    let dir = Dir::new("shapes-player");
    let root = dir.0.to_string_lossy().into_owned();
    let input = write_wav(&dir, "in.wav", 44_100, 2, 44_100 * 2);
    let mut player = std::ptr::null_mut();
    assert_eq!(unsafe { cf_player_new(48_000, 2, &mut player) }, CF_OK);
    let p = c(&input);
    assert_eq!(unsafe { cf_player_load(player, p.as_ptr()) }, CF_OK);
    assert_eq!(unsafe { cf_player_settle(player) }, CF_OK);
    let state = json_out(|out| unsafe { cf_player_state(player, out) });
    insta::assert_snapshot!(shown(state, &root));
    assert_eq!(unsafe { cf_player_destroy(player) }, CF_OK);
}
