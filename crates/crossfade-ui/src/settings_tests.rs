use appconfig::{SettingsEditor, SettingsLayer, ThemeMode, ThemeSelection, load_layers};

use crate::config_test_support::TempDir;
use crate::settings::{
    ConvertDefaults, DEFAULT_SETTINGS, IDENTITY, SETTINGS_TEMPLATE, Settings, clamp_ui_font_size,
};

/// The template with every commented-out member (`// "key": ...`, `// }`) live again.
fn uncomment(template: &str) -> String {
    let lines: Vec<String> = template
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            let indent = &line[..line.len() - trimmed.len()];
            let live = trimmed
                .strip_prefix("//")
                .map(|after| after.strip_prefix(' ').unwrap_or(after))
                .filter(|rest| {
                    let rest = rest.trim_start();
                    rest.starts_with('"') || rest.starts_with('}')
                });
            match live {
                Some(rest) => format!("{indent}{rest}"),
                None => line.to_owned(),
            }
        })
        .collect();
    lines.join("\n")
}

#[test]
fn the_defaults_file_is_the_default_settings() {
    let loaded = load_layers::<Settings>(DEFAULT_SETTINGS, &[]).expect("defaults parse");
    assert_eq!(loaded.value, Settings::default());
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
}

#[test]
fn defaults_follow_the_system_with_the_bundled_themes() {
    let settings = Settings::default();
    assert_eq!(
        settings.theme,
        ThemeSelection::Dynamic {
            mode: ThemeMode::System,
            light: Some("Crossfade Light".into()),
            dark: Some("Crossfade Dark".into()),
        }
    );
    assert_eq!(settings.ui_font_family, None);
    assert_eq!(settings.ui_font_size, 13.0);
    assert_eq!(
        settings.convert,
        ConvertDefaults {
            preset: "same".into(),
            overwrite: false
        }
    );
}

#[test]
fn the_template_as_shipped_changes_nothing() {
    let layer = SettingsLayer::new("settings.json", SETTINGS_TEMPLATE);
    let loaded = load_layers::<Settings>(DEFAULT_SETTINGS, &[layer]).expect("template parses");
    assert_eq!(loaded.value, Settings::default());
}

#[test]
fn the_template_uncommented_loads_without_warnings() {
    let layer = SettingsLayer::new("settings.json", uncomment(SETTINGS_TEMPLATE));
    let loaded = load_layers::<Settings>(DEFAULT_SETTINGS, &[layer]).expect("template parses");
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    assert_eq!(loaded.value, Settings::default());
}

#[test]
fn the_template_mentions_every_key_of_the_schema() {
    let value = serde_json::to_value(Settings::default()).unwrap();
    for key in value.as_object().unwrap().keys() {
        assert!(
            SETTINGS_TEMPLATE.contains(&format!("// \"{key}\"")),
            "the template lacks {key}"
        );
    }
}

#[test]
fn the_editor_uncomments_a_template_line_in_place() {
    let dir = TempDir::new("template-edit");
    let path = dir.write("settings.json", SETTINGS_TEMPLATE);
    let mut editor = SettingsEditor::open(&path, SETTINGS_TEMPLATE).expect("open");
    editor.set("/ui_font_size", 18.0).expect("set");
    editor.save().expect("save");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"ui_font_size\": 18"), "{text}");
    assert!(!text.contains("// \"ui_font_size\""), "{text}");
    let loaded = load_layers::<Settings>(DEFAULT_SETTINGS, &[SettingsLayer::new(&path, text)])
        .expect("edited file loads");
    assert_eq!(loaded.value.ui_font_size, 18.0);
}

#[test]
fn ui_font_size_is_clamped_to_a_drawable_range() {
    assert_eq!(clamp_ui_font_size(14.0), 14.0);
    assert_eq!(clamp_ui_font_size(2.0), 10.0);
    assert_eq!(clamp_ui_font_size(90.0), 24.0);
    assert_eq!(clamp_ui_font_size(f32::NAN), 13.0);
}

#[test]
fn the_identity_names_the_crossfade_config() {
    assert_eq!(IDENTITY.env_var, "CROSSFADE_CONFIG_DIR");
    assert_eq!(IDENTITY.unix_dir, "crossfade");
    assert_eq!(IDENTITY.windows_dir, "Crossfade");
    assert_eq!(IDENTITY.project_dir, ".crossfade");
    assert_eq!(IDENTITY.fallback_dir, "crossfade-config");
}
