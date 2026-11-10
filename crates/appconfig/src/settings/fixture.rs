//! A schema for the tests of the reusable settings pieces: the font and theme keys in
//! Zed's vocabulary, as an app would put them in its own settings type.

use serde::{Deserialize, Serialize};

use super::de::{font_size, font_weight, opt_font_size, opt_font_weight};
use super::font::{FontFeatures, LineHeight};
use super::load::{Loaded, SettingsError, SettingsLayer, load_layers};
use super::theme_selection::{ThemeMode, ThemeSelection};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Demo {
    #[serde(default)]
    pub(crate) buffer_font_family: Option<String>,
    #[serde(default = "default_size", deserialize_with = "font_size")]
    pub(crate) buffer_font_size: f32,
    #[serde(default = "default_weight", deserialize_with = "font_weight")]
    pub(crate) buffer_font_weight: f32,
    #[serde(default)]
    pub(crate) buffer_font_features: FontFeatures,
    #[serde(default)]
    pub(crate) buffer_line_height: LineHeight,
    #[serde(default)]
    pub(crate) terminal: Terminal,
    #[serde(default = "default_theme")]
    pub(crate) theme: ThemeSelection,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Terminal {
    #[serde(default, deserialize_with = "opt_font_size")]
    pub(crate) font_size: Option<f32>,
    #[serde(default, deserialize_with = "opt_font_weight")]
    pub(crate) font_weight: Option<f32>,
    #[serde(default)]
    pub(crate) line_height: Option<LineHeight>,
}

fn default_size() -> f32 {
    15.0
}

fn default_weight() -> f32 {
    400.0
}

fn default_theme() -> ThemeSelection {
    ThemeSelection::Dynamic {
        mode: ThemeMode::System,
        light: Some("Demo Light".into()),
        dark: Some("Demo Dark".into()),
    }
}

impl Default for Demo {
    fn default() -> Self {
        Self {
            buffer_font_family: None,
            buffer_font_size: default_size(),
            buffer_font_weight: default_weight(),
            buffer_font_features: FontFeatures::new(),
            buffer_line_height: LineHeight::Standard,
            terminal: Terminal::default(),
            theme: default_theme(),
        }
    }
}

pub(crate) const DEFAULTS: &str = r#"{
    "buffer_font_family": null,
    "buffer_font_size": 15.0,
    "buffer_font_weight": 400,
    "buffer_font_features": {},
    "buffer_line_height": "standard",
    "terminal": { "font_size": null, "font_weight": null, "line_height": null },
    "theme": { "mode": "system", "light": "Demo Light", "dark": "Demo Dark" },
}"#;

pub(crate) fn try_load(text: &str) -> Result<Loaded<Demo>, SettingsError> {
    load_layers(DEFAULTS, &[SettingsLayer::new("<user>", text)])
}

pub(crate) fn load(text: &str) -> Demo {
    try_load(text)
        .unwrap_or_else(|err| panic!("settings failed to load: {err}"))
        .value
}

pub(crate) fn error_of(text: &str) -> SettingsError {
    try_load(text).expect_err("settings should be rejected")
}

pub(crate) fn position(err: &SettingsError) -> (usize, usize) {
    match err {
        SettingsError::Invalid { line, column, .. } => (*line, *column),
        other => panic!("expected an Invalid error, got {other:?}"),
    }
}
