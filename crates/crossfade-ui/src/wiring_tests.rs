//! The configuration wired into the window: keymap, notices, the settings file, the sheet's
//! defaults and the theme. Every test runs over a temp config directory.

use crate::actions::{AppAction, OpenKeymap};
use crate::config::{self, Config, ConfigPiece};
use crate::config_test_support::TempDir as ConfigDir;
use crate::prelude::*;
use crate::settings::{KEYMAP_TEMPLATE, SETTINGS_TEMPLATE};
use crate::test_support::{
    add_paths, click, draw, has, open_rig, open_rig_in, open_sheet, opened, press, step,
};

/// The bundled keymap's binding for OpenFiles on this platform.
const OPEN_KEY: &str = if cfg!(target_os = "macos") {
    "cmd-o"
} else {
    "ctrl-o"
};

fn last_action(cx: &mut TestAppContext, rig: &crate::test_support::Rig) -> Option<AppAction> {
    rig.workspace.read_with(cx, |w, _| w.last_action())
}

fn notice(cx: &mut TestAppContext, rig: &crate::test_support::Rig) -> Option<String> {
    rig.workspace.read_with(cx, |w, cx| w.status_notice(cx))
}

#[gpui_test]
fn init_installs_a_bundled_config_before_any_window(cx: &mut TestAppContext) {
    cx.update(|cx| {
        crate::init(cx);
        let config = cx.try_global::<Config>().expect("a config is installed");
        assert_eq!(config.generation(), 1);
        assert!(config.problems().is_empty());
        assert!(
            cx.theme().is_dark(),
            "bundled dark until a config is started"
        );
        // The bundled keymap, not hard-coded bindings, is what is bound.
        let key = Keystroke::parse(OPEN_KEY).unwrap();
        let open = cx.all_bindings_for_input(&[key]);
        assert_eq!(open.len(), 1);
    });
}

#[gpui_test]
fn a_user_keymap_rebinding_open_files_takes_effect_in_the_window(cx: &mut TestAppContext) {
    let config = ConfigDir::new("keymap-window");
    config.write(
        "keymap.json",
        r#"[{ "context": "Workspace", "bindings": { "secondary-shift-o": "crossfade::OpenFiles" } }]"#,
    );
    let rig = open_rig_in(cx, config);
    draw(cx, rig.window);
    press(cx, rig.window, "secondary-shift-o");
    assert_eq!(last_action(cx, &rig), Some(AppAction::OpenFiles));
}

#[gpui_test]
fn a_broken_settings_file_shows_a_status_notice_until_it_is_fixed(cx: &mut TestAppContext) {
    let config = ConfigDir::new("notice-window");
    config.write("settings.json", "{ \"ui_font_size\": ");
    let rig = open_rig_in(cx, config);
    draw(cx, rig.window);
    assert!(has(cx, rig.window, "status-notice"));
    assert!(
        !has(cx, rig.window, "status-text"),
        "the notice replaces the normal text"
    );
    let line = notice(cx, &rig).expect("a notice");
    assert!(line.contains("settings.json"), "{line}");

    rig.config
        .write("settings.json", r#"{ "ui_font_size": 13 }"#);
    cx.update(|cx| config::reload(ConfigPiece::Settings, cx));
    draw(cx, rig.window);
    assert!(!has(cx, rig.window, "status-notice"));
    assert!(has(cx, rig.window, "status-text"));
}

#[gpui_test]
fn open_settings_creates_the_template_in_the_config_directory(cx: &mut TestAppContext) {
    // The test platform's `open_with_system` is unimplemented, so the app opens files
    // through `config::Opener`, which the test window records.
    let rig = open_rig(cx);
    draw(cx, rig.window);
    let path = rig.config.path().join("settings.json");
    assert!(!path.exists());
    press(cx, rig.window, "secondary-,");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), SETTINGS_TEMPLATE);
    assert_eq!(opened(cx), vec![path.clone()]);
    assert_eq!(last_action(cx, &rig), Some(AppAction::OpenSettings));

    std::fs::write(&path, "{}").unwrap();
    click(cx, rig.window, "toolbar-settings");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "{}",
        "never overwritten"
    );
}

#[gpui_test]
fn open_keymap_creates_the_keymap_template(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    draw(cx, rig.window);
    cx.update_window(rig.window, |_, window, cx| {
        window.dispatch_action(Box::new(OpenKeymap), cx);
    })
    .expect("open");
    cx.run_until_parked();
    let path = rig.config.path().join("keymap.json");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), KEYMAP_TEMPLATE);
    assert_eq!(opened(cx), vec![path]);
}

#[gpui_test]
fn a_failed_open_settings_shows_a_short_lived_notice(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    draw(cx, rig.window);
    // A config directory below a regular file cannot be created.
    let file = rig.config.write("not-a-directory", "x");
    let blocked = appconfig::ConfigPaths::new(file.join("config"));
    cx.update(|cx| config::install(Config::bundled(blocked), cx));
    press(cx, rig.window, "secondary-,");
    draw(cx, rig.window);
    assert!(has(cx, rig.window, "status-notice"));
    let line = notice(cx, &rig).expect("a notice");
    assert!(line.contains("settings"), "{line}");

    // It goes away by itself.
    step(cx, 1);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(10));
    cx.run_until_parked();
    draw(cx, rig.window);
    assert!(!has(cx, rig.window, "status-notice"));
}

#[gpui_test]
fn the_sheet_opens_with_the_configured_convert_defaults(cx: &mut TestAppContext) {
    let config = ConfigDir::new("sheet-defaults");
    config.write(
        "settings.json",
        r#"{ "convert": { "preset": "studio", "overwrite": true } }"#,
    );
    let rig = open_rig_in(cx, config);
    let input = rig.dir.write_wav("in.wav", 48_000, 1, 100);
    add_paths(cx, &rig, &[&input]);
    open_sheet(cx, &rig);
    rig.workspace.read_with(cx, |w, cx| {
        let dialog = w.convert_dialog().expect("the sheet is open").read(cx);
        assert_eq!(dialog.preset_label(cx), Some("24-bit"));
        assert_eq!(dialog.format_label(cx), "24-bit integer");
        assert!(dialog.overwrite());
    });
}

#[gpui_test]
fn an_unknown_preset_falls_back_to_same_as_source(cx: &mut TestAppContext) {
    let config = ConfigDir::new("sheet-unknown");
    config.write(
        "settings.json",
        r#"{ "convert": { "preset": "nonsense" } }"#,
    );
    let rig = open_rig_in(cx, config);
    let input = rig.dir.write_wav("in.wav", 48_000, 1, 100);
    add_paths(cx, &rig, &[&input]);
    open_sheet(cx, &rig);
    rig.workspace.read_with(cx, |w, cx| {
        let dialog = w.convert_dialog().expect("the sheet is open").read(cx);
        assert_eq!(dialog.preset_label(cx), Some("Same as source"));
        assert!(!dialog.overwrite());
    });
}

#[gpui_test]
fn switching_the_theme_changes_the_workspace_background(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    draw(cx, rig.window);
    let dark = rig.workspace.read_with(cx, |w, cx| w.surface_color(cx));
    rig.config
        .write("settings.json", r#"{ "theme": "Crossfade Light" }"#);
    cx.update(|cx| config::reload(ConfigPiece::Settings, cx));
    draw(cx, rig.window);
    let light = rig.workspace.read_with(cx, |w, cx| w.surface_color(cx));
    assert_ne!(dark, light);
    assert!(light.l > dark.l, "light {light:?} dark {dark:?}");
}

#[gpui_test]
fn a_flac_preset_in_settings_opens_the_sheet_as_flac(cx: &mut TestAppContext) {
    let config = ConfigDir::new("sheet-flac");
    config.write("settings.json", r#"{ "convert": { "preset": "flac-24" } }"#);
    let rig = open_rig_in(cx, config);
    let input = rig.dir.write_wav("in.wav", 48_000, 1, 100);
    add_paths(cx, &rig, &[&input]);
    open_sheet(cx, &rig);
    rig.workspace.read_with(cx, |w, cx| {
        let dialog = w.convert_dialog().expect("the sheet is open").read(cx);
        assert_eq!(dialog.preset_label(cx), Some("FLAC 24-bit"));
        assert_eq!(dialog.container_label(cx), "FLAC");
        assert_eq!(dialog.format_label(cx), "24-bit integer");
        assert_eq!(dialog.output(cx), rig.dir.path("in-s24.flac"));
    });
}
