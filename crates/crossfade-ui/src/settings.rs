//! The settings schema, its bundled defaults and the files a new config starts from.

use appconfig::{AppIdentity, ThemeDefaults, ThemeMode, ThemeSelection};
use serde::{Deserialize, Serialize};

/// What names Crossfade's config directory and project folder.
pub const IDENTITY: AppIdentity = AppIdentity {
    env_var: "CROSSFADE_CONFIG_DIR",
    unix_dir: "crossfade",
    windows_dir: "Crossfade",
    project_dir: ".crossfade",
    fallback_dir: "crossfade-config",
};

/// The base layer of every settings load: all keys, with comments.
pub const DEFAULT_SETTINGS: &str = include_str!("../assets/default_settings.jsonc");

/// A new user `settings.json`: every key commented out.
pub const SETTINGS_TEMPLATE: &str = include_str!("../assets/settings_template.jsonc");

/// A new user `keymap.json`.
pub const KEYMAP_TEMPLATE: &str = include_str!("../assets/keymap_template.json");

/// The themes the app falls back to for each system appearance.
pub const THEME_DEFAULTS: ThemeDefaults = ThemeDefaults {
    dark: "Crossfade Dark",
    light: "Crossfade Light",
};

/// The bundled theme families as `(origin, text)`.
pub const BUNDLED_THEMES: &[(&str, &str)] = &[(
    "<bundled crossfade.json>",
    include_str!("../assets/themes/crossfade.json"),
)];

const DEFAULT_UI_FONT_SIZE: f32 = 14.0;
const MIN_UI_FONT_SIZE: f32 = 10.0;
const MAX_UI_FONT_SIZE: f32 = 24.0;

/// The size the interface is drawn at: out-of-range values are clamped so a typo cannot
/// make the window unreadable, and a non-finite one falls back to the default.
pub fn clamp_ui_font_size(size: f32) -> f32 {
    if size.is_finite() {
        size.clamp(MIN_UI_FONT_SIZE, MAX_UI_FONT_SIZE)
    } else {
        DEFAULT_UI_FONT_SIZE
    }
}

/// What a new conversion starts from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConvertDefaults {
    pub preset: String,
    pub overwrite: bool,
}

impl Default for ConvertDefaults {
    fn default() -> Self {
        Self {
            preset: "same".into(),
            overwrite: false,
        }
    }
}

/// Everything `settings.json` can set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeSelection,
    /// `None` is gpui-kit's default family.
    pub ui_font_family: Option<String>,
    pub ui_font_size: f32,
    pub convert: ConvertDefaults,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeSelection::Dynamic {
                mode: ThemeMode::System,
                light: Some(THEME_DEFAULTS.light.into()),
                dark: Some(THEME_DEFAULTS.dark.into()),
            },
            ui_font_family: None,
            ui_font_size: DEFAULT_UI_FONT_SIZE,
            convert: ConvertDefaults::default(),
        }
    }
}
