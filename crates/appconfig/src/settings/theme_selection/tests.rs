use super::*;
use crate::settings::fixture::{DEFAULTS, Demo, error_of, load};
use crate::settings::{SettingsLayer, load_layers};

fn dynamic(mode: ThemeMode, light: Option<&str>, dark: Option<&str>) -> ThemeSelection {
    ThemeSelection::Dynamic {
        mode,
        light: light.map(Into::into),
        dark: dark.map(Into::into),
    }
}

#[test]
fn a_theme_string_replaces_a_theme_object_and_back() {
    let loaded = load_layers::<Demo>(
        DEFAULTS,
        &[
            SettingsLayer::new(
                "user",
                r#"{ "theme": { "mode": "light", "light": "Mine" } }"#,
            ),
            SettingsLayer::new("project", r#"{ "theme": "Pinned" }"#),
        ],
    )
    .unwrap();
    assert_eq!(loaded.value.theme, ThemeSelection::Static("Pinned".into()));
    assert_eq!(loaded.value.theme.name(true), Some("Pinned"));
    assert_eq!(loaded.value.theme.name(false), Some("Pinned"));

    let user_only = load_layers::<Demo>(
        DEFAULTS,
        &[SettingsLayer::new(
            "user",
            r#"{ "theme": { "mode": "light", "light": "Mine" } }"#,
        )],
    )
    .unwrap();
    // The object merged with the default one, so `dark` keeps its default.
    assert_eq!(
        user_only.value.theme,
        dynamic(ThemeMode::Light, Some("Mine"), Some("Demo Dark"))
    );
}

#[test]
fn a_theme_object_may_leave_names_out() {
    // Read on its own, a partial object has no name for the other appearance: the app
    // falls back to its default theme for it.
    let theme: ThemeSelection =
        serde_json::from_str(r#"{ "mode": "system", "dark": "Night" }"#).unwrap();
    assert_eq!(theme, dynamic(ThemeMode::System, None, Some("Night")));
    assert_eq!(theme.name(true), Some("Night"));
    assert_eq!(theme.name(false), None);
    // A bare object is the system mode.
    let bare: ThemeSelection = serde_json::from_str("{}").unwrap();
    assert_eq!(bare, dynamic(ThemeMode::System, None, None));
}

#[test]
fn the_mode_decides_which_appearance_applies() {
    let system = load(r#"{ "theme": { "mode": "system", "dark": "Night" } }"#).theme;
    assert!(system.prefers_dark(true));
    assert!(!system.prefers_dark(false));
    assert_eq!(system.name(true), Some("Night"));
    assert_eq!(system.name(false), Some("Demo Light"));

    let light = load(r#"{ "theme": { "mode": "light" } }"#).theme;
    assert!(!light.prefers_dark(true));
    let dark = load(r#"{ "theme": { "mode": "dark" } }"#).theme;
    assert!(dark.prefers_dark(false));
    assert_eq!(dark.name(false), Some("Demo Dark"));

    // A plain name pins one theme, so it follows the system for the fallback only.
    let pinned = ThemeSelection::Static("X".into());
    assert!(pinned.prefers_dark(true));
    assert!(!pinned.prefers_dark(false));
}

#[test]
fn bad_theme_values_are_errors() {
    assert!(
        error_of(r#"{ "theme": { "mode": "dim" } }"#)
            .to_string()
            .contains("dim")
    );
    assert!(
        error_of(r#"{ "theme": 3 }"#)
            .to_string()
            .contains("theme name")
    );
    assert!(error_of(r#"{ "theme": { "light": 3 } }"#).file().is_some());
}

#[test]
fn unknown_keys_in_a_theme_object_are_warned_about_by_the_loader() {
    let loaded = load_layers::<Demo>(
        DEFAULTS,
        &[SettingsLayer::new(
            "user",
            r#"{ "theme": { "mode": "dark", "extra": 1 } }"#,
        )],
    )
    .unwrap();
    let keys: Vec<&str> = loaded.warnings.iter().map(|w| w.key.as_str()).collect();
    assert_eq!(keys, ["theme.extra"]);
}

#[test]
fn a_selection_round_trips_through_json() {
    for theme in [
        ThemeSelection::Static("Named".into()),
        dynamic(ThemeMode::Dark, Some("L"), Some("D")),
        dynamic(ThemeMode::System, None, Some("D")),
    ] {
        let json = serde_json::to_value(&theme).unwrap();
        let back: ThemeSelection = serde_json::from_value(json).unwrap();
        assert_eq!(back, theme);
    }
}
