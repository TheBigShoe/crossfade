use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::*;
use crate::test_util::TempDir;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Display {
    #[serde(default)]
    wrap: bool,
    #[serde(default)]
    mode: Mode,
}

impl Default for Display {
    fn default() -> Self {
        Self {
            wrap: false,
            mode: Mode::Text,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Mode {
    #[default]
    Text,
    Hex,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Item {
    #[serde(default)]
    name: String,
    #[serde(default)]
    rate: Option<u32>,
}

/// The fixture schema. `items` holds one sample element, which is how the loader learns
/// the shape of an array's elements for the unknown-key check.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Demo {
    #[serde(default)]
    size: u32,
    #[serde(default)]
    family: Option<String>,
    #[serde(default)]
    display: Display,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    items: Vec<Item>,
    #[serde(default)]
    features: BTreeMap<String, u32>,
}

impl Default for Demo {
    fn default() -> Self {
        Self {
            size: 15,
            family: None,
            display: Display::default(),
            tags: vec!["a".into()],
            items: vec![Item::default()],
            features: BTreeMap::new(),
        }
    }
}

const DEFAULTS: &str = r#"// bundled defaults
{
    "size": 15,
    "family": null,
    "display": { "wrap": false, "mode": "text" },
    "tags": ["a"],
    "items": [],
    "features": {},
}"#;

fn load(text: &str) -> Loaded<Demo> {
    load_layers(DEFAULTS, &[SettingsLayer::new("<user>", text)])
        .unwrap_or_else(|err| panic!("settings failed to load: {err}"))
}

fn error_of(text: &str) -> SettingsError {
    load_layers::<Demo>(DEFAULTS, &[SettingsLayer::new("<user>", text)])
        .expect_err("settings should be rejected")
}

fn position(err: &SettingsError) -> (usize, usize) {
    match err {
        SettingsError::Invalid { line, column, .. } => (*line, *column),
        other => panic!("expected an Invalid error, got {other:?}"),
    }
}

#[test]
fn no_layers_gives_the_defaults() {
    let loaded = load_layers::<Demo>(DEFAULTS, &[]).unwrap();
    assert_eq!(loaded.value.size, 15);
    assert_eq!(loaded.value.tags, ["a"]);
    assert!(loaded.value.items.is_empty());
    assert!(loaded.warnings.is_empty());
}

#[test]
fn comments_and_trailing_commas_are_accepted() {
    let text = "// leading\n{ /* block */ \"size\": 14, // tail\n \"items\": [],\n}\n// end";
    assert_eq!(load(text).value.size, 14);
    let comments_only = "// nothing to see\n/* here */\n";
    assert_eq!(load(comments_only).value.size, 15);
    assert_eq!(load("").value.size, 15);
}

#[test]
fn a_non_object_root_is_rejected() {
    let err = error_of("[1, 2]");
    assert!(err.to_string().contains("JSON object"), "{err}");
}

#[test]
fn layering_is_defaults_then_user_then_project() {
    let dir = TempDir::new("layers");
    let user = dir.write(
        "user.json",
        r#"{
            "size": 13,
            "family": "User Mono",
            "display": { "wrap": true, "mode": "hex" },
            "tags": ["x", "y"],
            "items": [ { "name": "a", "rate": 9600 } ]
        }"#,
    );
    let project = dir.write(
        "project.json",
        r#"{
            "size": 11,
            "display": { "mode": "text" },
            "tags": ["z"],
            "items": [ { "name": "b", "rate": null } ],
            "family": null
        }"#,
    );

    let user_only = load_files::<Demo>(DEFAULTS, Some(&user), None)
        .unwrap()
        .value;
    assert_eq!(user_only.size, 13);
    assert_eq!(user_only.family.as_deref(), Some("User Mono"));

    let both = load_files::<Demo>(DEFAULTS, Some(&user), Some(&project))
        .unwrap()
        .value;
    // A later layer wins for scalars.
    assert_eq!(both.size, 11);
    // Objects merge key by key: the project changed the mode only, the user's wrap
    // survives.
    assert_eq!(
        both.display,
        Display {
            wrap: true,
            mode: Mode::Text
        }
    );
    // `null` unsets an optional key.
    assert_eq!(both.family, None);
    // Arrays replace rather than concatenate.
    assert_eq!(both.tags, ["z"]);
    assert_eq!(
        both.items,
        [Item {
            name: "b".into(),
            rate: None
        }]
    );
}

#[test]
fn a_missing_file_adds_no_layer() {
    let dir = TempDir::new("missing");
    let missing = dir.path().join("nope.json");
    let loaded = load_files::<Demo>(DEFAULTS, Some(&missing), Some(&missing)).unwrap();
    assert_eq!(loaded.value.size, 15);
    assert!(load_files::<Demo>(DEFAULTS, None, None).is_ok());
}

#[test]
fn a_read_error_names_the_file() {
    let dir = TempDir::new("unreadable");
    // A directory is not a readable settings file.
    let err = load_files::<Demo>(DEFAULTS, Some(dir.path()), None).unwrap_err();
    assert!(matches!(err, SettingsError::Io { .. }), "{err:?}");
    assert_eq!(err.file(), Some(dir.path()));
}

#[test]
fn errors_carry_file_line_column_and_message() {
    let dir = TempDir::new("errors");
    let bad_syntax = dir.write("syntax.json", "{\n  \"size\": ,\n}\n");
    let err = load_files::<Demo>(DEFAULTS, Some(&bad_syntax), None).unwrap_err();
    match &err {
        SettingsError::Invalid {
            file,
            line,
            column,
            message,
        } => {
            assert_eq!(file, &bad_syntax);
            assert_eq!(*line, 2);
            assert!(*column >= 1);
            assert!(!message.is_empty());
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
    assert_eq!(err.file(), Some(bad_syntax.as_path()));
    assert!(
        err.to_string()
            .starts_with(&bad_syntax.display().to_string())
    );

    let bad_type = dir.write("type.json", "{\n  \"size\": \"big\",\n}\n");
    let err = load_files::<Demo>(DEFAULTS, None, Some(&bad_type)).unwrap_err();
    let SettingsError::Invalid {
        file,
        line,
        column,
        message,
    } = &err
    else {
        panic!("expected Invalid, got {err:?}");
    };
    assert_eq!(file, &bad_type);
    assert_eq!((*line, *column), (2, 11));
    assert!(message.contains("u32"), "{message}");

    // The error names the file that is wrong even when an earlier layer is fine.
    let good = dir.write("good.json", "{ \"size\": 12 }");
    let err = load_files::<Demo>(DEFAULTS, Some(&good), Some(&bad_type)).unwrap_err();
    assert_eq!(err.file(), Some(bad_type.as_path()));

    let bad_enum = dir.write(
        "enum.json",
        "{\n \"display\": {\n  \"mode\": \"binary\"\n }\n}",
    );
    let err = load_files::<Demo>(DEFAULTS, Some(&bad_enum), None).unwrap_err();
    assert_eq!(position(&err).0, 3);
    assert!(err.to_string().contains("binary"), "{err}");
}

#[test]
fn null_unsets_optional_keys_but_not_ones_with_a_default() {
    let loaded = load(r#"{ "family": null }"#);
    assert_eq!(loaded.value.family, None);
    assert!(matches!(
        error_of(r#"{ "size": null }"#),
        SettingsError::Invalid { .. }
    ));
}

#[test]
fn unknown_keys_are_warnings_not_errors() {
    let dir = TempDir::new("unknown");
    let path = dir.write(
        "settings.json",
        r#"{
            "sizee": 1,
            "display": { "wrapp": true, "wrap": true },
            "items": [ { "name": "n", "bogus": 2 }, { "rat": 1 } ],
            "features": { "anything": 1 },
            "family": "x",
            "some_other_apps_setting": { "a": 1 }
        }"#,
    );
    let loaded = load_files::<Demo>(DEFAULTS, Some(&path), None).unwrap();
    assert!(loaded.value.display.wrap);
    let mut keys: Vec<&str> = loaded.warnings.iter().map(|w| w.key.as_str()).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "display.wrapp",
            "items[0].bogus",
            "items[1].rat",
            "sizee",
            "some_other_apps_setting",
        ]
    );
    let warning = loaded
        .warnings
        .iter()
        .find(|w| w.key == "sizee")
        .expect("a warning for sizee");
    assert_eq!(warning.file, path);
    assert!(warning.message.contains("`sizee`"));
    assert!(warning.to_string().contains("unknown setting"));
}

#[test]
fn warnings_name_the_layer_they_came_from() {
    let loaded = load_layers::<Demo>(
        DEFAULTS,
        &[
            SettingsLayer::new("user.json", r#"{ "one": 1 }"#),
            SettingsLayer::new("project.json", r#"{ "two": 2 }"#),
        ],
    )
    .unwrap();
    let origins: Vec<(PathBuf, &str)> = loaded
        .warnings
        .iter()
        .map(|w| (w.file.clone(), w.key.as_str()))
        .collect();
    assert_eq!(
        origins,
        [
            (PathBuf::from("user.json"), "one"),
            (PathBuf::from("project.json"), "two"),
        ]
    );
}

#[test]
fn known_keys_produce_no_warnings() {
    let loaded = load(
        r#"{ "size": 1, "family": "F", "display": { "wrap": true, "mode": "hex" },
             "tags": [], "items": [ { "name": "n", "rate": 1 } ], "features": { "x": 1 } }"#,
    );
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
}

#[test]
fn defaults_that_do_not_parse_are_an_error() {
    let err = load_layers::<Demo>("{ \"size\": }", &[]).unwrap_err();
    assert!(matches!(err, SettingsError::Invalid { .. }), "{err:?}");
}

#[test]
fn defaults_that_do_not_fit_the_schema_are_a_merged_error() {
    let err = load_layers::<Demo>(r#"{ "size": "large" }"#, &[]).unwrap_err();
    assert!(matches!(err, SettingsError::Merged(_)), "{err:?}");
    assert_eq!(err.file(), None);
}
