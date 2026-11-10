use appconfig::{Keymap, Platform};
use std::path::Path;

use crate::actions::AppAction;
use crate::keymap::{KitBindings, apply, bundled_keymap_text, snapshot_kit_bindings};
use crate::prelude::*;
use crate::test_support::{open_test_window, press};

const PLATFORMS: [Platform; 3] = [Platform::MacOs, Platform::Linux, Platform::Windows];

fn bundled(platform: Platform) -> Keymap {
    Keymap::parse(bundled_keymap_text(platform), Path::new("bundled")).expect("bundled keymap")
}

fn user(text: &str) -> Keymap {
    let mut keymap = bundled(Platform::MacOs);
    keymap.extend(Keymap::parse(text, Path::new("keymap.json")).expect("user keymap"));
    keymap
}

/// The window with the app initialised, and no key bindings but the ones `keymap` brings.
fn open_with(
    cx: &mut TestAppContext,
    keymap: &Keymap,
) -> (AnyWindowHandle, Entity<crate::Workspace>, Vec<String>) {
    let (window, workspace) = open_test_window(cx);
    let problems = cx.update(|cx| {
        // `init` binds its own defaults; the snapshot of kit's bindings belongs to the
        // wiring in `init`, so here the kit part is empty.
        cx.set_global(KitBindings::default());
        apply(keymap, cx)
    });
    (window, workspace, problems)
}

fn last_action(cx: &mut TestAppContext, workspace: &Entity<crate::Workspace>) -> Option<AppAction> {
    workspace.read_with(cx, |workspace, _| workspace.last_action())
}

#[gpui_test]
fn every_bundled_binding_names_an_action_that_can_be_built(cx: &mut TestAppContext) {
    cx.update(crate::workspace::init);
    cx.update(|cx| {
        // Actions with no default binding still have to be buildable by name, for a user
        // keymap to bind them.
        for name in ["crossfade::About", "crossfade::OpenKeymap"] {
            cx.build_action(name, None).expect(name);
        }
        for platform in PLATFORMS {
            let keymap = bundled(platform);
            assert!(!keymap.entries.is_empty(), "{platform:?} binds nothing");
            for entry in &keymap.entries {
                let Some(action) = &entry.action else {
                    continue;
                };
                cx.build_action(&action.name, action.args.clone())
                    .unwrap_or_else(|error| {
                        panic!(
                            "{platform:?} {}: {}: {error}",
                            entry.keystrokes, action.name
                        )
                    });
            }
        }
    });
}

#[test]
fn each_platform_binds_the_same_actions_to_its_own_modifier() {
    let expected = [
        ("o", "crossfade::OpenFiles"),
        ("enter", "crossfade::Convert"),
        ("j", "crossfade::ToggleQueue"),
        (",", "crossfade::OpenSettings"),
        ("q", "crossfade::Quit"),
    ];
    for platform in PLATFORMS {
        let modifier = if platform == Platform::MacOs {
            "cmd"
        } else {
            "ctrl"
        };
        let keymap = bundled(platform);
        let resolved = keymap.resolved();
        for (key, action) in expected {
            let keystrokes = format!("{modifier}-{key}");
            let found = resolved
                .iter()
                .find(|binding| binding.keystrokes == keystrokes)
                .unwrap_or_else(|| panic!("{platform:?} does not bind {keystrokes}"));
            assert_eq!(found.action.as_ref().map(|a| a.name.as_str()), Some(action));
        }
        for unbound in ["crossfade::About", "crossfade::OpenKeymap"] {
            assert!(
                !keymap.action_names().contains(&unbound),
                "{unbound} has no default binding"
            );
        }
    }
}

#[gpui_test]
fn the_bundled_keymap_drives_the_window(cx: &mut TestAppContext) {
    let (window, workspace, problems) = open_with(cx, &bundled(Platform::MacOs));
    assert_eq!(problems, Vec::<String>::new());
    press(cx, window, "cmd-o");
    assert_eq!(last_action(cx, &workspace), Some(AppAction::OpenFiles));
    press(cx, window, "cmd-enter");
    assert_eq!(last_action(cx, &workspace), Some(AppAction::Convert));
    press(cx, window, "cmd-j");
    assert_eq!(last_action(cx, &workspace), Some(AppAction::ToggleQueue));
    press(cx, window, "cmd-,");
    assert_eq!(last_action(cx, &workspace), Some(AppAction::OpenSettings));
}

#[gpui_test]
fn a_user_binding_overrides_a_bundled_one(cx: &mut TestAppContext) {
    let keymap =
        user(r#"[{ "context": "Workspace", "bindings": { "cmd-o": "crossfade::ToggleQueue" } }]"#);
    let (window, workspace, problems) = open_with(cx, &keymap);
    assert_eq!(problems, Vec::<String>::new());
    press(cx, window, "cmd-o");
    assert_eq!(last_action(cx, &workspace), Some(AppAction::ToggleQueue));
}

#[gpui_test]
fn a_user_null_unbinds_a_bundled_key(cx: &mut TestAppContext) {
    let keymap = user(r#"[{ "context": "Workspace", "bindings": { "cmd-o": null } }]"#);
    let (window, workspace, problems) = open_with(cx, &keymap);
    assert_eq!(problems, Vec::<String>::new());
    press(cx, window, "cmd-o");
    assert_eq!(last_action(cx, &workspace), None, "cmd-o is unbound");
    press(cx, window, "cmd-j");
    assert_eq!(last_action(cx, &workspace), Some(AppAction::ToggleQueue));
}

#[gpui_test]
fn an_unknown_action_is_a_problem_and_the_rest_still_binds(cx: &mut TestAppContext) {
    let keymap = user(
        r#"[{ "context": "Workspace", "bindings": {
            "cmd-o": "nowhere::Missing",
            "cmd-shift-j": "crossfade::ToggleQueue"
        } }]"#,
    );
    let (window, workspace, problems) = open_with(cx, &keymap);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("nowhere::Missing"), "{problems:?}");
    assert!(problems[0].contains("cmd-o"), "{problems:?}");
    press(cx, window, "cmd-shift-j");
    assert_eq!(last_action(cx, &workspace), Some(AppAction::ToggleQueue));
    // The bundled binding the bad entry tried to replace is still there.
    press(cx, window, "cmd-o");
    assert_eq!(last_action(cx, &workspace), Some(AppAction::OpenFiles));
}

#[gpui_test]
fn a_bad_keystroke_or_context_is_a_problem(cx: &mut TestAppContext) {
    let keymap = user(
        r#"[
            { "context": "Workspace", "bindings": { "hyper-nonsense-": "crossfade::Convert" } },
            { "context": "Workspace >", "bindings": { "cmd-k": "crossfade::Convert" } }
        ]"#,
    );
    let (_window, _workspace, problems) = open_with(cx, &keymap);
    assert_eq!(problems.len(), 2, "{problems:?}");
}

#[gpui_test]
fn applying_twice_does_not_bind_twice(cx: &mut TestAppContext) {
    let keymap = bundled(Platform::MacOs);
    let (_window, _workspace, _) = open_with(cx, &keymap);
    let count =
        |cx: &mut TestAppContext| cx.update(|cx| cx.key_bindings().borrow().bindings().len());
    let once = count(cx);
    assert!(once > 0);
    cx.update(|cx| apply(&keymap, cx));
    assert_eq!(count(cx), once);
}

#[gpui_test]
fn kits_own_bindings_survive_every_apply(cx: &mut TestAppContext) {
    cx.update(|cx| {
        kit_init(cx);
        snapshot_kit_bindings(cx);
        let kit = cx.global::<KitBindings>().0.len();
        assert!(kit > 0, "gpui-kit binds keys of its own");
        apply(&bundled(Platform::MacOs), cx);
        let after = cx.key_bindings().borrow().bindings().len();
        assert!(after > kit);
        apply(&Keymap::default(), cx);
        assert_eq!(cx.key_bindings().borrow().bindings().len(), kit);
    });
}
