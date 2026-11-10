//! The `theme` setting in Zed's form: a theme name, or a mode with a theme per
//! appearance.

use std::fmt;

use serde::de::{self, Deserializer, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Serialize, Serializer};

/// Whether a `theme` object follows the system appearance or pins one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

/// The `theme` setting: a theme name, or `{ "mode", "light", "dark" }`.
///
/// A name left out of the object is `None`; the app uses its default theme for that
/// appearance (see `ThemeRegistry::resolve`). When the app's bundled defaults hold a
/// full object, a user file that sets only `mode` still gets both names, because the
/// layers merge before the value is read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThemeSelection {
    /// One theme regardless of the system appearance.
    Static(String),
    Dynamic {
        mode: ThemeMode,
        light: Option<String>,
        dark: Option<String>,
    },
}

impl ThemeSelection {
    /// Whether the dark variant applies given the system appearance. A plain theme
    /// name pins one theme, so it follows the system for the fallback choice only.
    pub fn prefers_dark(&self, system_dark: bool) -> bool {
        match self {
            ThemeSelection::Static(_) => system_dark,
            ThemeSelection::Dynamic { mode, .. } => match mode {
                ThemeMode::System => system_dark,
                ThemeMode::Light => false,
                ThemeMode::Dark => true,
            },
        }
    }

    /// The theme name this selection asks for under the given system appearance, or
    /// `None` when it names none for that appearance.
    pub fn name(&self, system_dark: bool) -> Option<&str> {
        match self {
            ThemeSelection::Static(name) => Some(name),
            ThemeSelection::Dynamic { light, dark, .. } => {
                if self.prefers_dark(system_dark) {
                    dark.as_deref()
                } else {
                    light.as_deref()
                }
            }
        }
    }
}

impl Serialize for ThemeSelection {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Object<'a> {
            mode: ThemeMode,
            // Written as null rather than left out, so the serialized default lists
            // every key the loader accepts.
            light: &'a Option<String>,
            dark: &'a Option<String>,
        }

        match self {
            ThemeSelection::Static(name) => s.serialize_str(name),
            ThemeSelection::Dynamic { mode, light, dark } => Object {
                mode: *mode,
                light,
                dark,
            }
            .serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for ThemeSelection {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ThemeVisitor;

        impl<'de> Visitor<'de> for ThemeVisitor {
            type Value = ThemeSelection;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a theme name or { \"mode\", \"light\", \"dark\" }")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<ThemeSelection, E> {
                Ok(ThemeSelection::Static(v.to_string()))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<ThemeSelection, A::Error> {
                let mut mode = None;
                let mut light = None;
                let mut dark = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "mode" => mode = Some(map.next_value::<ThemeMode>()?),
                        "light" => light = map.next_value::<Option<String>>()?,
                        "dark" => dark = map.next_value::<Option<String>>()?,
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(ThemeSelection::Dynamic {
                    mode: mode.unwrap_or_default(),
                    light,
                    dark,
                })
            }
        }

        deserializer.deserialize_any(ThemeVisitor)
    }
}

#[cfg(test)]
mod tests;
