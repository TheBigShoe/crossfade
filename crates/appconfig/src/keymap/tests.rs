use std::path::Path;

use serde_json::json;

use crate::test_util::TempDir;

use super::*;

fn parse(text: &str) -> Keymap {
    Keymap::parse(text, Path::new("keymap.json")).unwrap_or_else(|err| panic!("{err}"))
}

fn parse_err(text: &str) -> KeymapError {
    Keymap::parse(text, Path::new("keymap.json")).expect_err("keymap should be rejected")
}

fn position(err: &KeymapError) -> (usize, usize) {
    match err {
        KeymapError::Invalid { line, column, .. } => (*line, *column),
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn string_array_and_null_actions() {
    let keymap = parse(
        r#"[
            {
                "context": "Terminal",
                "bindings": {
                    "cmd-k": "terminal::Clear",
                    "cmd-p": ["terminal::Pause", { "arg": 1 }],
                    "cmd-l": ["terminal::JumpToBottom"],
                    "ctrl-x": null
                }
            }
        ]"#,
    );
    assert_eq!(
        keymap.entries,
        vec![
            KeyBinding::bind(Some("Terminal"), "cmd-k", "terminal::Clear"),
            KeyBinding {
                context: Some("Terminal".into()),
                keystrokes: "cmd-p".into(),
                action: Some(ActionRef {
                    name: "terminal::Pause".into(),
                    args: Some(json!({ "arg": 1 })),
                }),
                use_key_equivalents: false,
            },
            KeyBinding::bind(Some("Terminal"), "cmd-l", "terminal::JumpToBottom"),
            KeyBinding::unbind(Some("Terminal"), "ctrl-x"),
        ]
    );
    assert!(keymap.entries[3].action.is_none());
}

#[test]
fn action_arguments_can_be_any_json() {
    let keymap = parse(
        r#"[ { "bindings": {
            "a": ["x::A", 5],
            "b": ["x::B", "text"],
            "c": ["x::C", [1, 2, { "k": null }]],
            "d": ["x::D", null],
            "e": ["x::E", false]
        } } ]"#,
    );
    let args: Vec<_> = keymap
        .entries
        .iter()
        .map(|entry| entry.action.as_ref().unwrap().args.clone())
        .collect();
    assert_eq!(
        args,
        vec![
            Some(json!(5)),
            Some(json!("text")),
            Some(json!([1, 2, { "k": null }])),
            // A null argument is no argument.
            None,
            Some(json!(false)),
        ]
    );
}

#[test]
fn sections_without_a_context_bind_everywhere() {
    let keymap = parse(
        r#"[ { "bindings": { "cmd-q": "app::Quit" } },
              { "context": "", "bindings": { "cmd-w": "session::Close" } },
              { "context": null, "bindings": { "cmd-e": "compose::CycleLineEnding" } },
              { "context": "  Workspace ", "bindings": { "cmd-r": "terminal::ToggleRecord" } } ]"#,
    );
    let contexts: Vec<_> = keymap
        .entries
        .iter()
        .map(|e| e.context.as_deref())
        .collect();
    assert_eq!(contexts, vec![None, None, None, Some("Workspace")]);
}

#[test]
fn use_key_equivalents_is_carried_per_section() {
    let keymap = parse(
        r#"[ { "use_key_equivalents": true, "bindings": { "cmd-a": "terminal::SelectAll" } },
              { "use_key_equivalents": false, "bindings": { "cmd-b": "x::B" } },
              { "bindings": { "cmd-c": "x::C" } } ]"#,
    );
    let flags: Vec<_> = keymap
        .entries
        .iter()
        .map(|e| e.use_key_equivalents)
        .collect();
    assert_eq!(flags, vec![true, false, false]);
}

#[test]
fn file_order_is_kept_including_repeated_keystrokes() {
    let keymap = parse(
        r#"[ { "bindings": { "z": "x::Z", "a": "x::A", "m": "x::M", "a": "x::A2" } },
              { "context": "Terminal", "bindings": { "b": "x::B" } } ]"#,
    );
    let order: Vec<_> = keymap
        .entries
        .iter()
        .map(|e| {
            (
                e.keystrokes.as_str(),
                e.action.as_ref().unwrap().name.as_str(),
            )
        })
        .collect();
    assert_eq!(
        order,
        vec![
            ("z", "x::Z"),
            ("a", "x::A"),
            ("m", "x::M"),
            ("a", "x::A2"),
            ("b", "x::B")
        ]
    );
}

#[test]
fn multi_key_chords_are_kept_as_written() {
    let keymap =
        parse(r#"[ { "bindings": { "ctrl-x ctrl-c": "app::Quit", " cmd-k ": "x::K" } } ]"#);
    assert_eq!(keymap.entries[0].keystrokes, "ctrl-x ctrl-c");
    // Surrounding whitespace is trimmed.
    assert_eq!(keymap.entries[1].keystrokes, "cmd-k");
}

#[test]
fn comments_trailing_commas_and_empty_files() {
    let keymap = parse(
        "// my keys\n[\n  { /* section */ \"bindings\": { \"cmd-k\": \"terminal::Clear\", }, },\n]\n",
    );
    assert_eq!(keymap.entries.len(), 1);
    for empty in [
        "",
        "  \n",
        "// only a comment\n",
        "[]",
        "null",
        "[ { } ]",
        "[ { \"bindings\": null } ]",
    ] {
        assert_eq!(parse(empty), Keymap::default(), "{empty:?}");
    }
}

#[test]
fn errors_carry_file_line_and_column() {
    // A binding of the wrong type points at the value.
    let err = parse_err(
        "[\n  { \"context\": \"Terminal\",\n    \"bindings\": {\n      \"cmd-k\": 5\n    } }\n]",
    );
    let (line, column) = position(&err);
    assert_eq!((line, column), (4, 16));
    assert!(err.to_string().starts_with("keymap.json:4:16: "), "{err}");
    assert!(err.to_string().contains("an action name"), "{err}");

    // Bad syntax.
    let err = parse_err("[\n  { \"bindings\": { \"cmd-k\": } }\n]");
    assert_eq!(position(&err).0, 2);

    // The root must be a list.
    let err = parse_err("{ \"bindings\": {} }");
    assert!(err.to_string().contains("a keymap: a list"), "{err}");

    for (text, needle) in [
        (r#"[ { "bindings": { "cmd-k": "" } } ]"#, "cannot be empty"),
        (
            r#"[ { "bindings": { "": "x::A" } } ]"#,
            "keystroke cannot be empty",
        ),
        (
            r#"[ { "bindings": { "cmd-k": [] } } ]"#,
            "action name first",
        ),
        (r#"[ { "bindings": { "cmd-k": [1] } } ]"#, "string"),
        (
            r#"[ { "bindings": { "cmd-k": ["x::A", 1, 2] } } ]"#,
            "[action name, arguments]",
        ),
        (r#"[ { "bindings": { "cmd-k": true } } ]"#, "an action name"),
        (
            r#"[ { "bindings": { "cmd-k": { "a": 1 } } } ]"#,
            "an action name",
        ),
        (r#"[ { "bindings": [] } ]"#, "keystrokes to actions"),
        (r#"[ { "context": 5, "bindings": {} } ]"#, "string"),
        (r#"[ 5 ]"#, "struct"),
    ] {
        let err = parse_err(text);
        assert!(err.to_string().contains(needle), "{text}: {err}");
    }
}

/// The app's bundled defaults, as the loader is handed them.
const BUNDLED: &str = r#"// bundled defaults
[
  { "bindings": { "cmd-q": "app::Quit" } },
  { "context": "Workspace",
    "bindings": { "cmd-k": "terminal::Clear", "cmd-p": "palette::Open", "cmd-s": "terminal::Export" } },
  { "context": "Terminal", "bindings": { "cmd-f": "terminal::Search" } },
]"#;

#[test]
fn a_user_keymap_is_appended_after_the_defaults() {
    let dir = TempDir::new("keymap");
    let user = dir.write(
        "keymap.json",
        r#"[ { "context": "Workspace",
               "bindings": { "cmd-k": "terminal::JumpToBottom", "cmd-p": null, "cmd-alt-1": "x::New" } } ]"#,
    );
    let defaults = parse(BUNDLED);
    assert_eq!(defaults.entries.len(), 5);
    let keymap = load_keymap(BUNDLED, Some(&user)).unwrap();
    assert_eq!(keymap.entries.len(), defaults.entries.len() + 3);
    // Defaults first, untouched.
    assert_eq!(
        keymap.entries[..defaults.entries.len()],
        defaults.entries[..]
    );
    // The user's entries follow, in file order.
    let tail = &keymap.entries[defaults.entries.len()..];
    assert_eq!(
        tail[0],
        KeyBinding::bind(Some("Workspace"), "cmd-k", "terminal::JumpToBottom")
    );
    assert_eq!(tail[1], KeyBinding::unbind(Some("Workspace"), "cmd-p"));
    assert_eq!(
        tail[2],
        KeyBinding::bind(Some("Workspace"), "cmd-alt-1", "x::New")
    );

    // Applying in order, the user's bindings win and the unbind removes the default.
    let resolved = keymap.resolved();
    let find = |context: &str, keys: &str| {
        resolved
            .iter()
            .find(|b| b.context.as_deref() == Some(context) && b.keystrokes == keys)
            .and_then(|b| b.action.as_ref().map(|a| a.name.as_str()))
    };
    assert_eq!(find("Workspace", "cmd-k"), Some("terminal::JumpToBottom"));
    assert_eq!(find("Workspace", "cmd-p"), None);
    assert_eq!(find("Workspace", "cmd-alt-1"), Some("x::New"));
    assert_eq!(find("Workspace", "cmd-s"), Some("terminal::Export"));
    // The same keystroke in another context is a different binding.
    assert_eq!(find("Terminal", "cmd-f"), Some("terminal::Search"));
    assert_eq!(resolved.len(), defaults.entries.len() + 1 - 1);
}

#[test]
fn load_keymap_handles_missing_and_broken_files() {
    let dir = TempDir::new("keymap-errors");
    let missing = dir.path().join("nope.json");
    let defaults = parse(BUNDLED);
    assert_eq!(load_keymap(BUNDLED, Some(&missing)).unwrap(), defaults);
    assert_eq!(load_keymap(BUNDLED, None).unwrap(), defaults);

    let broken = dir.write("broken.json", "[ { \"bindings\": { \"cmd-k\": 5 } } ]");
    match load_keymap(BUNDLED, Some(&broken)).unwrap_err() {
        KeymapError::Invalid { file, line, .. } => {
            assert_eq!(file, broken);
            assert_eq!(line, 1);
        }
        other => panic!("expected Invalid, got {other:?}"),
    }

    // A directory where the file should be is an I/O error, not a silent default.
    let err = load_keymap(BUNDLED, Some(dir.path())).unwrap_err();
    assert!(matches!(err, KeymapError::Io { .. }), "{err:?}");
}

#[test]
fn broken_bundled_text_is_an_error_naming_it_not_a_panic() {
    let err = load_keymap("[ { \"bindings\": { \"a\": 5 } } ]", None).unwrap_err();
    let KeymapError::Invalid { file, .. } = err else {
        panic!("expected Invalid");
    };
    assert_eq!(file, Path::new("<bundled keymap>"));
}

#[test]
fn resolved_keeps_the_last_entry_per_context_and_keystroke() {
    let mut keymap = Keymap::default();
    keymap.entries.push(KeyBinding::bind(None, "a", "x::One"));
    keymap
        .entries
        .push(KeyBinding::bind(Some("T"), "a", "x::Two"));
    keymap.entries.push(KeyBinding::bind(None, "b", "x::B"));
    keymap.entries.push(KeyBinding::bind(None, "a", "x::Three"));
    keymap.entries.push(KeyBinding::unbind(None, "b"));
    let names: Vec<_> = keymap
        .resolved()
        .iter()
        .map(|b| {
            (
                b.context.clone(),
                b.keystrokes.clone(),
                b.action.as_ref().unwrap().name.clone(),
            )
        })
        .collect();
    assert_eq!(
        names,
        vec![
            (Some("T".to_string()), "a".to_string(), "x::Two".to_string()),
            (None, "a".to_string(), "x::Three".to_string()),
        ]
    );
    assert_eq!(
        keymap.action_names(),
        vec!["x::One", "x::Two", "x::B", "x::Three"]
    );
}
