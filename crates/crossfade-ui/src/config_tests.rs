//! The configuration applied to the running app: the real loaders from `appconfig` over
//! temporary config directories, installed into headless apps.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use crate::config::{
    self, Config, ConfigPiece, missing_family, open_keymap_file, open_settings_file,
};
use crate::config_test_support::TempDir;
use crate::prelude::*;
use crate::settings::{BUNDLED_THEMES, KEYMAP_TEMPLATE, SETTINGS_TEMPLATE};
use crate::theme_bridge::to_hsla;

fn init(cx: &mut TestAppContext) {
    cx.update(|cx| {
        kit_init(cx);
        crate::keymap::snapshot_kit_bindings(cx);
    });
}

/// Load `dir` with the real loaders and install the result, as a reload does.
fn load(cx: &mut TestAppContext, dir: &TempDir, system_dark: bool) {
    let paths = dir.paths();
    cx.update(|cx| config::install(Config::load(&paths, system_dark), cx));
    cx.run_until_parked();
}

fn reload(cx: &mut TestAppContext, piece: ConfigPiece) {
    cx.update(|cx| config::reload(piece, cx));
    cx.run_until_parked();
}

fn config(cx: &mut TestAppContext) -> Config {
    cx.update(|cx| cx.global::<Config>().clone())
}

fn notice(cx: &mut TestAppContext) -> Option<String> {
    config(cx).notice()
}

fn kit_background(cx: &mut TestAppContext) -> [u8; 4] {
    cx.update(|cx| bytes(cx.theme().background))
}

fn bytes(color: Hsla) -> [u8; 4] {
    let rgba = color.to_rgb();
    [rgba.r, rgba.g, rgba.b, rgba.a].map(|v| (v * 255.0).round() as u8)
}

/// The `background` of a bundled theme, as the kit shows it.
fn bundled_background(name: &str) -> [u8; 4] {
    let registry =
        appconfig::ThemeRegistry::with_bundled(BUNDLED_THEMES, crate::settings::THEME_DEFAULTS)
            .expect("bundled themes");
    bytes(to_hsla(
        registry.get(name).unwrap().color("background").unwrap(),
    ))
}

#[gpui_test]
fn the_bundled_config_installs(cx: &mut TestAppContext) {
    init(cx);
    let dir = TempDir::new("bundled");
    let paths = dir.paths();
    cx.update(|cx| config::install(Config::bundled(paths), cx));
    let config = config(cx);
    assert_eq!(config.theme().name, "Crossfade Dark");
    assert_eq!(config.generation(), 1);
    assert_eq!(config.notice(), None);
    assert!(config.problems().is_empty());
    assert_eq!(kit_background(cx), bundled_background("Crossfade Dark"));
    assert!(cx.update(|cx| cx.theme().is_dark()));
    assert!(
        !dir.path().join("settings.json").exists(),
        "nothing is written"
    );
}

#[gpui_test]
fn an_empty_config_directory_loads_the_defaults_cleanly(cx: &mut TestAppContext) {
    init(cx);
    let dir = TempDir::new("empty");
    load(cx, &dir, false);
    let config = config(cx);
    assert_eq!(config.theme().name, "Crossfade Light");
    assert_eq!(config.notice(), None);
    assert_eq!(**config.settings(), crate::settings::Settings::default());
}

#[gpui_test]
fn a_user_settings_file_selecting_light_switches_the_theme(cx: &mut TestAppContext) {
    init(cx);
    let dir = TempDir::new("light");
    dir.write("settings.json", r#"{ "theme": "Crossfade Light" }"#);
    load(cx, &dir, true);
    assert_eq!(config(cx).theme().name, "Crossfade Light");
    assert_eq!(kit_background(cx), bundled_background("Crossfade Light"));
    assert!(!cx.update(|cx| cx.theme().is_dark()));
}

#[gpui_test]
fn ui_font_settings_reach_gpui_kit_and_the_size_is_clamped(cx: &mut TestAppContext) {
    init(cx);
    let dir = TempDir::new("font");
    dir.write("settings.json", r#"{ "ui_font_size": 90 }"#);
    load(cx, &dir, true);
    assert_eq!(cx.update(|cx| cx.theme().font_size), px(24.));
    dir.write("settings.json", r#"{ "ui_font_size": 2 }"#);
    reload(cx, ConfigPiece::Settings);
    assert_eq!(cx.update(|cx| cx.theme().font_size), px(10.));
    dir.write("settings.json", r#"{ "ui_font_size": 18 }"#);
    reload(cx, ConfigPiece::Settings);
    assert_eq!(cx.update(|cx| cx.theme().font_size), px(18.));
    // The settings still hold what the user wrote; only the applied size is clamped.
    assert_eq!(config(cx).settings().ui_font_size, 18.0);
}

#[gpui_test]
fn a_broken_settings_file_keeps_the_last_good_settings_and_says_so(cx: &mut TestAppContext) {
    init(cx);
    let dir = TempDir::new("broken");
    dir.write("settings.json", r#"{ "ui_font_size": 18 }"#);
    load(cx, &dir, true);
    assert_eq!(notice(cx), None);

    dir.write("settings.json", "{ \"ui_font_size\": ");
    reload(cx, ConfigPiece::Settings);
    let after = config(cx);
    assert_eq!(
        after.settings().ui_font_size,
        18.0,
        "the last good value stays"
    );
    assert_eq!(cx.update(|cx| cx.theme().font_size), px(18.));
    let notice = after.notice().expect("a notice");
    assert!(notice.contains("settings.json"), "{notice}");
    assert!(!notice.contains('\n'), "one line: {notice}");
    assert_eq!(after.problems().len(), 1);
    assert_eq!(after.problems()[0].file, dir.path().join("settings.json"));

    dir.write("settings.json", r#"{ "ui_font_size": 20 }"#);
    reload(cx, ConfigPiece::Settings);
    assert_eq!(notice(cx), None);
    assert_eq!(config(cx).settings().ui_font_size, 20.0);
}

#[gpui_test]
fn a_broken_first_load_falls_back_to_the_bundled_settings(cx: &mut TestAppContext) {
    init(cx);
    let dir = TempDir::new("broken-first");
    dir.write("settings.json", "not json at all");
    load(cx, &dir, true);
    assert_eq!(
        **config(cx).settings(),
        crate::settings::Settings::default()
    );
    assert!(notice(cx).is_some());
}

#[gpui_test]
fn an_unknown_setting_is_a_warning_not_a_failure(cx: &mut TestAppContext) {
    init(cx);
    let dir = TempDir::new("unknown-key");
    dir.write(
        "settings.json",
        r#"{ "ui_font_size": 16, "ui_fnt_size": 3 }"#,
    );
    load(cx, &dir, true);
    assert_eq!(config(cx).settings().ui_font_size, 16.0);
    let notice = notice(cx).expect("the typo is reported");
    assert!(notice.contains("ui_fnt_size"), "{notice}");
}

#[gpui_test]
fn a_theme_file_in_the_themes_folder_can_be_selected_by_name(cx: &mut TestAppContext) {
    init(cx);
    let dir = TempDir::new("user-theme");
    let family = BUNDLED_THEMES[0]
        .1
        .replace("Crossfade Dark", "Mine Dark")
        .replace("Crossfade Light", "Mine Light")
        .replacen("#16181d", "#102030", 1);
    dir.write("themes/mine.json", &family);
    dir.write("settings.json", r#"{ "theme": "Mine Dark" }"#);
    load(cx, &dir, false);
    let config = config(cx);
    assert_eq!(config.theme().name, "Mine Dark");
    assert_eq!(config.notice(), None);
    assert_eq!(kit_background(cx), [0x10, 0x20, 0x30, 0xff]);
}

#[gpui_test]
fn a_theme_file_saved_later_is_picked_up_by_a_themes_reload(cx: &mut TestAppContext) {
    init(cx);
    let dir = TempDir::new("theme-later");
    dir.write("settings.json", r#"{ "theme": "Mine Dark" }"#);
    load(cx, &dir, true);
    assert_eq!(
        config(cx).theme().name,
        "Crossfade Dark",
        "falls back while missing"
    );
    assert!(
        notice(cx)
            .expect("missing theme is reported")
            .contains("Mine Dark")
    );

    let family = BUNDLED_THEMES[0]
        .1
        .replace("Crossfade Dark", "Mine Dark")
        .replace("Crossfade Light", "Mine Light");
    dir.write("themes/mine.json", &family);
    reload(cx, ConfigPiece::Themes);
    assert_eq!(config(cx).theme().name, "Mine Dark");
    assert_eq!(notice(cx), None);
}

#[gpui_test]
fn a_broken_theme_file_is_skipped_and_reported(cx: &mut TestAppContext) {
    init(cx);
    let dir = TempDir::new("bad-theme");
    dir.write("themes/bad.json", "{ nope");
    load(cx, &dir, true);
    assert_eq!(config(cx).theme().name, "Crossfade Dark");
    let notice = notice(cx).expect("reported");
    assert!(notice.contains("bad.json"), "{notice}");
}

#[gpui_test]
fn the_system_appearance_flips_a_system_following_selection(cx: &mut TestAppContext) {
    init(cx);
    let dir = TempDir::new("appearance");
    load(cx, &dir, true);
    assert_eq!(config(cx).theme().name, "Crossfade Dark");
    let generation = config(cx).generation();

    cx.update(|cx| config::set_appearance(false, cx));
    assert_eq!(config(cx).theme().name, "Crossfade Light");
    assert!(!cx.update(|cx| cx.theme().is_dark()));
    assert_eq!(kit_background(cx), bundled_background("Crossfade Light"));
    assert_eq!(config(cx).generation(), generation + 1);

    // The same appearance again installs nothing.
    cx.update(|cx| config::set_appearance(false, cx));
    assert_eq!(config(cx).generation(), generation + 1);

    cx.update(|cx| config::set_appearance(true, cx));
    assert_eq!(config(cx).theme().name, "Crossfade Dark");
}

#[gpui_test]
fn a_pinned_theme_ignores_the_system_appearance(cx: &mut TestAppContext) {
    init(cx);
    let dir = TempDir::new("pinned");
    dir.write("settings.json", r#"{ "theme": "Crossfade Light" }"#);
    load(cx, &dir, true);
    cx.update(|cx| config::set_appearance(false, cx));
    cx.update(|cx| config::set_appearance(true, cx));
    assert_eq!(config(cx).theme().name, "Crossfade Light");
}

#[gpui_test]
fn set_appearance_and_reload_do_nothing_before_a_config_is_installed(cx: &mut TestAppContext) {
    init(cx);
    cx.update(|cx| {
        config::set_appearance(false, cx);
        config::reload(ConfigPiece::Settings, cx);
        assert!(cx.try_global::<Config>().is_none());
    });
}

#[gpui_test]
fn every_install_notifies_observers_and_bumps_the_generation(cx: &mut TestAppContext) {
    init(cx);
    let dir = TempDir::new("observe");
    load(cx, &dir, true);
    let seen = Rc::new(Cell::new(0u32));
    cx.update(|cx| {
        let seen = seen.clone();
        cx.observe_global::<Config>(move |_| seen.set(seen.get() + 1))
            .detach();
    });
    let before = config(cx).generation();
    reload(cx, ConfigPiece::Settings);
    reload(cx, ConfigPiece::Themes);
    assert_eq!(seen.get(), 2);
    assert_eq!(config(cx).generation(), before + 2);
}

#[gpui_test]
fn a_user_keymap_rebinds_and_a_bad_entry_is_a_notice(cx: &mut TestAppContext) {
    init(cx);
    let dir = TempDir::new("keymap");
    dir.write(
        "keymap.json",
        r#"[{ "context": "Workspace", "bindings": {
            "cmd-shift-j": "crossfade::ToggleQueue",
            "cmd-shift-k": "nowhere::Missing"
        } }]"#,
    );
    load(cx, &dir, true);
    let notice = notice(cx).expect("the unknown action is reported");
    assert!(notice.contains("nowhere::Missing"), "{notice}");
    let bound = cx.update(|cx| {
        cx.all_bindings_for_input(&[Keystroke::parse("cmd-shift-j").unwrap()])
            .len()
    });
    assert_eq!(bound, 1, "the good entry bound");
}

#[gpui_test]
fn a_broken_keymap_keeps_the_last_good_bindings(cx: &mut TestAppContext) {
    init(cx);
    let dir = TempDir::new("keymap-broken");
    dir.write(
        "keymap.json",
        r#"[{ "context": "Workspace", "bindings": { "cmd-shift-j": "crossfade::ToggleQueue" } }]"#,
    );
    load(cx, &dir, true);
    assert_eq!(notice(cx), None);

    dir.write("keymap.json", "[ { ");
    reload(cx, ConfigPiece::Keymap);
    let notice = notice(cx).expect("a notice");
    assert!(notice.contains("keymap.json"), "{notice}");
    let bound = cx.update(|cx| {
        cx.all_bindings_for_input(&[Keystroke::parse("cmd-shift-j").unwrap()])
            .len()
    });
    assert_eq!(bound, 1, "the last good binding stays");

    dir.write("keymap.json", "[]");
    reload(cx, ConfigPiece::Keymap);
    assert_eq!(notice(cx), None);
}

#[gpui_test]
fn several_problems_show_the_first_and_a_count(cx: &mut TestAppContext) {
    init(cx);
    let dir = TempDir::new("several");
    dir.write("settings.json", "{ ");
    dir.write("themes/bad.json", "{ nope");
    load(cx, &dir, true);
    let notice = notice(cx).expect("a notice");
    assert!(notice.contains("(+1 more)"), "{notice}");
    // An error (a file that did not load) outranks a warning.
    assert!(notice.contains("settings.json"), "{notice}");
}

#[test]
fn paths_for_an_explicit_directory_uses_it() {
    let dir = TempDir::new("paths-for");
    let paths = config::paths_for(Some(dir.path().to_path_buf()));
    assert_eq!(paths.dir, dir.path());
    assert_eq!(paths.settings, dir.path().join("settings.json"));
    assert_eq!(paths.keymap, dir.path().join("keymap.json"));
    assert_eq!(paths.themes, dir.path().join("themes"));
}

#[test]
fn open_settings_file_creates_the_template_once_and_never_overwrites() {
    let dir = TempDir::new("open-settings");
    let paths = dir.paths();
    let path = open_settings_file(&paths).expect("created");
    assert_eq!(path, dir.path().join("settings.json"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), SETTINGS_TEMPLATE);

    std::fs::write(&path, r#"{ "ui_font_size": 20 }"#).unwrap();
    assert_eq!(open_settings_file(&paths).unwrap(), path);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        r#"{ "ui_font_size": 20 }"#
    );
}

#[test]
fn open_settings_file_creates_the_config_directory() {
    let dir = TempDir::new("open-settings-nested");
    let paths = appconfig::ConfigPaths::new(dir.path().join("a").join("b"));
    let path = open_settings_file(&paths).expect("created");
    assert!(path.is_file());
}

#[gpui_test]
fn the_keymap_template_loads_without_problems(cx: &mut TestAppContext) {
    init(cx);
    let dir = TempDir::new("open-keymap");
    let paths = dir.paths();
    let path = open_keymap_file(&paths).expect("created");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), KEYMAP_TEMPLATE);
    std::fs::write(&path, "[]").unwrap();
    assert_eq!(open_keymap_file(&paths).unwrap(), path);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "[]",
        "not overwritten"
    );

    std::fs::write(&path, KEYMAP_TEMPLATE).unwrap();
    load(cx, &dir, true);
    assert_eq!(notice(cx), None);
}

#[gpui_test]
fn the_settings_template_as_created_loads_without_problems(cx: &mut TestAppContext) {
    init(cx);
    let dir = TempDir::new("template-load");
    open_settings_file(&dir.paths()).expect("created");
    load(cx, &dir, true);
    assert_eq!(notice(cx), None);
    assert_eq!(
        **config(cx).settings(),
        crate::settings::Settings::default()
    );
}

#[test]
fn a_family_is_missing_only_when_fonts_are_listed_and_it_is_not_among_them() {
    let installed = ["Inter".to_owned(), "Menlo".to_owned()];
    assert!(!missing_family("inter", &installed), "case-insensitive");
    assert!(missing_family("Berkeley Mono", &installed));
    assert!(
        !missing_family(".SystemUIFont", &installed),
        "gpui's own families"
    );
    assert!(
        !missing_family("Berkeley Mono", &[]),
        "a text system that cannot list fonts"
    );
}

#[gpui_test]
fn a_missing_ui_font_family_is_replaced_and_reported(cx: &mut TestAppContext) {
    init(cx);
    let dir = TempDir::new("fonts-missing");
    dir.write("settings.json", r#"{ "ui_font_family": "No Such Family" }"#);
    let paths = dir.paths();
    let mut loaded = Config::load(&paths, true);
    loaded.substitute_missing_fonts(&["Menlo".to_owned()]);
    cx.update(|cx| config::install(loaded, cx));
    assert_eq!(
        cx.update(|cx| cx.theme().font_family.to_string()),
        crate::theme_bridge::SYSTEM_UI_FAMILY
    );
    let notice = notice(cx).expect("reported");
    assert!(notice.contains("No Such Family"), "{notice}");
}

#[gpui_test]
fn the_watcher_reloads_a_saved_settings_file(cx: &mut TestAppContext) {
    // One real-watcher test; the rest of the watcher is covered by appconfig's own tests.
    cx.executor().allow_parking();
    init(cx);
    let dir = TempDir::new("watch");
    dir.write("settings.json", r#"{ "ui_font_size": 15 }"#);
    let paths = dir.paths();
    cx.update(|cx| config::start(paths, cx));
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.theme().font_size), px(15.));

    dir.write("settings.json", r#"{ "ui_font_size": 20 }"#);
    // Bounded by turns of the loop, not by the clock; the assertion is that it happens.
    let mut changed = false;
    for _ in 0..400 {
        cx.run_until_parked();
        if cx.update(|cx| cx.theme().font_size) == px(20.) {
            changed = true;
            break;
        }
        cx.executor().advance_clock(Duration::from_millis(8));
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(changed, "the saved font size never reached the theme");
    assert_eq!(notice(cx), None);
}
