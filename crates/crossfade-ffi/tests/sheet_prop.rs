//! Any sequence of changes through the C functions leaves a sheet whose "can_queue" agrees with
//! the engine: what can be queued, the engine validates and the command line parses back to
//! the same request; what the engine refuses cannot be queued.
#![allow(clippy::undocumented_unsafe_blocks)]

mod common;

use common::{Dir, c, fixture, json_out, write_wav};
use crossfade_app::FsIo;
use crossfade_app::inputs::probe_path;
use crossfade_cli::request_from_args;
use crossfade_core::Limits;
use crossfade_ffi::*;
use crossfade_graph::{Job, validate};
use proptest::prelude::*;
use serde_json::{Value, json};
use std::sync::OnceLock;

fn one_of(items: &'static [&'static str]) -> impl Strategy<Value = &'static str> {
    proptest::sample::select(items)
}

fn change() -> impl Strategy<Value = Value> {
    let texts = &[
        "", "-3", "x", "-16", "+40", "0.5", "1:00", "2", "500ms", "-1000",
    ];
    prop_oneof![
        one_of(&["wav", "flac", "aiff", "oga", "m4a", "opus"])
            .prop_map(|v| json!({"container": v})),
        one_of(&["same", "u8", "s16", "s24", "s32", "f32", "f64"])
            .prop_map(|v| json!({"format": v})),
        proptest::option::of(0u8..=10).prop_map(|v| json!({"compression_level": v})),
        proptest::option::of(proptest::sample::select(
            &[6_000u32, 32_000, 96_000, 300_000, 600_000][..]
        ))
        .prop_map(|v| json!({"bitrate": v})),
        proptest::option::of(proptest::sample::select(
            &[8_000u32, 22_050, 44_100, 48_000, 96_000, 384_000][..]
        ))
        .prop_map(|v| json!({"sample_rate": v})),
        proptest::option::of(one_of(&["mono", "stereo", "5.1", "quad", "3", "7.1"]))
            .prop_map(|v| json!({"channels": v})),
        any::<bool>().prop_map(|v| json!({"copy": v})),
        one_of(&[
            "same", "cd", "studio", "float", "flac", "flac-cd", "flac-24", "aiff", "alac", "opus"
        ])
        .prop_map(|v| json!({"preset": v})),
        one_of(&[
            "out.wav", "out.flac", "out.opus", "out.m4a", "out.mp3", "out", "out.ogg"
        ])
        .prop_map(|v| json!({"output": format!("@/{v}")})),
        any::<bool>().prop_map(|v| json!({"overwrite": v})),
        any::<bool>().prop_map(|v| json!({"adjust_open": v})),
        one_of(&["off", "gain", "normalize"]).prop_map(|v| json!({"level_mode": v})),
        (
            one_of(&[
                "level_value",
                "trim_start",
                "trim_end",
                "fade_in",
                "fade_out"
            ]),
            one_of(texts)
        )
            .prop_map(|(k, v)| json!({ k: v })),
    ]
}

struct Inputs {
    _dir: Dir,
    root: String,
    paths: Vec<String>,
}

fn inputs() -> &'static Inputs {
    static INPUTS: OnceLock<Inputs> = OnceLock::new();
    INPUTS.get_or_init(|| {
        let dir = Dir::new("sheet-prop");
        let paths = vec![
            write_wav(&dir, "stereo.wav", 48_000, 2, 9_600),
            write_wav(&dir, "mono.wav", 44_100, 1, 4_410),
            fixture("wav/f32-5.1.wav"),
            fixture("mp3/cbr128.mp3"),
            fixture("flac/stereo-24bit.flac"),
        ];
        Inputs {
            root: dir.0.to_string_lossy().into_owned(),
            _dir: dir,
            paths,
        }
    })
}

fn placed(v: &Value, root: &str) -> Value {
    match v.as_object() {
        Some(o) => o
            .iter()
            .map(|(k, x)| {
                let x = match x.as_str() {
                    Some(s) if k == "output" => json!(s.replace('@', root)),
                    _ => x.clone(),
                };
                (k.clone(), x)
            })
            .collect(),
        None => v.clone(),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    #[test]
    fn can_queue_agrees_with_the_engine(
        input in 0usize..5,
        changes in proptest::collection::vec(change(), 0..12),
    ) {
        let all = inputs();
        let path = &all.paths[input];
        let p = c(path);
        let mut sheet = std::ptr::null_mut();
        prop_assert_eq!(unsafe { cf_sheet_new(p.as_ptr(), &mut sheet) }, CF_OK);
        for change in &changes {
            let text = c(&placed(change, &all.root).to_string());
            prop_assert_eq!(unsafe { cf_sheet_apply(sheet, text.as_ptr()) }, CF_OK);
        }
        let state = json_out(|out| unsafe { cf_sheet_state(sheet, out) });
        unsafe { cf_sheet_destroy(sheet) };

        let job: Job = serde_json::from_value(state["request"]["job"].clone()).unwrap();
        let request = crossfade_app::Request {
            job,
            overwrite: state["request"]["overwrite"].as_bool().unwrap(),
        };
        let output = &request.job.outputs[0];
        let stream = probe_path(path, &mut FsIo::new(), &Limits::default());
        let stream = match stream {
            crossfade_app::inputs::InputState::Probed(_, s) => s.unwrap(),
            other => panic!("{other:?}"),
        };
        let engine = validate(&stream, &output.path, &output.audio);
        let can_queue = state["can_queue"].as_bool().unwrap();
        if can_queue {
            prop_assert!(engine.is_ok(), "queueable but refused: {:?}\n{}", engine, state["problems"]);
            let args = crossfade_app::command_args(&request).unwrap();
            let typed = request_from_args(&args);
            prop_assert!(matches!(&typed, Ok(Some(r)) if *r == request), "{args:?}: {typed:?}");
        }
        if engine.is_err() {
            prop_assert!(!can_queue);
        }
    }
}
