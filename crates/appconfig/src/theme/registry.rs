//! The set of themes available to the app: the bundled defaults plus the user's files.

use std::collections::HashMap;
use std::path::Path;

use parking_lot::Mutex;

use crate::paths::ConfigPaths;
use crate::settings::ThemeSelection;

use super::family::{Appearance, Theme, ThemeError, ThemeFamily, ThemeWarning};

/// Where a theme came from, for the override warning.
const BUNDLED_SOURCE: &str = "<bundled>";

struct Entry {
    theme: Theme,
    source: String,
}

/// The names of the themes the app falls back to for each appearance. Both must be among
/// its bundled themes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemeDefaults {
    /// The fallback for a dark appearance.
    pub dark: &'static str,
    /// The fallback for a light appearance.
    pub light: &'static str,
}

/// All loaded themes, looked up by name.
///
/// The registry is immutable once loaded and is `Sync`, so views can share it. When the
/// themes folder changes, load a new one.
pub struct ThemeRegistry {
    entries: Vec<Entry>,
    defaults: ThemeDefaults,
    by_name: HashMap<String, usize>,
    warnings: Mutex<Vec<ThemeWarning>>,
}

impl ThemeRegistry {
    /// A registry of the app's bundled theme families, given as `(origin, JSON text)` in
    /// the order [`names`](Self::names) lists them, with the names of its default themes.
    /// Text that is not a theme family, or a default name that no bundled theme has, is
    /// an error: both are fixed when the app is built.
    pub fn with_bundled(
        families: &[(&str, &str)],
        defaults: ThemeDefaults,
    ) -> Result<Self, ThemeError> {
        let mut registry = Self {
            entries: Vec::new(),
            defaults,
            by_name: HashMap::new(),
            warnings: Mutex::new(Vec::new()),
        };
        for (origin, text) in families {
            let (family, warnings) = ThemeFamily::parse(text, origin)?;
            if !warnings.is_empty() {
                tracing::warn!(?warnings, "bundled theme {origin} has warnings");
            }
            for theme in family.themes {
                registry.insert_theme(theme, BUNDLED_SOURCE);
            }
        }
        for name in [defaults.dark, defaults.light] {
            if !registry.by_name.contains_key(name) {
                return Err(ThemeError::MissingDefault {
                    name: name.to_string(),
                });
            }
        }
        Ok(registry)
    }

    /// The bundled themes plus every `*.json` file in `themes_dir`, in file name order.
    ///
    /// A user theme with the name of an earlier one replaces it, so a file can restyle
    /// a bundled theme. A file that cannot be read or parsed is skipped and reported in
    /// [`warnings`](Self::warnings); a missing directory is fine.
    pub fn load(
        families: &[(&str, &str)],
        defaults: ThemeDefaults,
        themes_dir: Option<&Path>,
    ) -> Result<Self, ThemeError> {
        let mut registry = Self::with_bundled(families, defaults)?;
        if let Some(dir) = themes_dir {
            registry.add_directory(dir);
        }
        Ok(registry)
    }

    /// [`load`](Self::load) with the themes folder of `paths`.
    pub fn load_from(
        families: &[(&str, &str)],
        defaults: ThemeDefaults,
        paths: &ConfigPaths,
    ) -> Result<Self, ThemeError> {
        Self::load(families, defaults, Some(&paths.themes))
    }

    /// Adds every `*.json` theme family file in `dir`, in file name order, as
    /// [`load`](Self::load) does.
    pub fn add_directory(&mut self, dir: &Path) {
        let read = match std::fs::read_dir(dir) {
            Ok(read) => read,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
            Err(err) => {
                self.warn(ThemeWarning {
                    source: dir.display().to_string(),
                    theme: None,
                    message: format!("cannot read the themes folder: {err}"),
                });
                return;
            }
        };
        let mut files: Vec<_> = read
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
            })
            .collect();
        files.sort();
        for path in files {
            let origin = path.display().to_string();
            let outcome = std::fs::read_to_string(&path)
                .map_err(|source| ThemeError::Io {
                    file: origin.clone(),
                    source,
                })
                .and_then(|text| self.add_family_text(&text, &origin));
            if let Err(err) = outcome {
                self.warn(ThemeWarning {
                    source: origin,
                    theme: None,
                    message: err.to_string(),
                });
            }
        }
    }

    /// Adds the themes of a theme family file, replacing any with the same names.
    /// Warnings from the file are kept in [`warnings`](Self::warnings).
    pub fn add_family_text(&mut self, text: &str, origin: &str) -> Result<(), ThemeError> {
        let (family, warnings) = ThemeFamily::parse(text, origin)?;
        for warning in warnings {
            self.warn(warning);
        }
        self.add_family(family, origin);
        Ok(())
    }

    /// Adds a parsed family. A theme replaces an earlier one of the same name.
    pub fn add_family(&mut self, family: ThemeFamily, source: &str) {
        for theme in family.themes {
            self.add_theme(theme, source);
        }
    }

    /// Adds one theme, replacing an earlier one of the same name.
    pub fn add_theme(&mut self, theme: Theme, source: &str) {
        self.insert_theme(theme, source);
    }

    fn insert_theme(&mut self, theme: Theme, source: &str) {
        match self.by_name.get(&theme.name).copied() {
            Some(index) => {
                let previous = &self.entries[index].source;
                if previous != BUNDLED_SOURCE {
                    let warning = ThemeWarning {
                        source: source.to_string(),
                        theme: Some(theme.name.clone()),
                        message: format!("replaces the theme of the same name from {previous}"),
                    };
                    self.warn(warning);
                }
                self.entries[index] = Entry {
                    theme,
                    source: source.to_string(),
                };
            }
            None => {
                self.by_name.insert(theme.name.clone(), self.entries.len());
                self.entries.push(Entry {
                    theme,
                    source: source.to_string(),
                });
            }
        }
    }

    /// The theme called `name`.
    pub fn get(&self, name: &str) -> Option<&Theme> {
        self.by_name
            .get(name)
            .map(|index| &self.entries[*index].theme)
    }

    /// Theme names, bundled first and then in the order they were loaded.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|entry| entry.theme.name.as_str())
    }

    /// Every theme, in the order of [`names`](Self::names).
    pub fn themes(&self) -> impl Iterator<Item = &Theme> {
        self.entries.iter().map(|entry| &entry.theme)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The default theme for an appearance. If a user file replaced it, the
    /// replacement.
    pub fn default_for(&self, appearance: Appearance) -> &Theme {
        let name = match appearance {
            Appearance::Dark => self.defaults.dark,
            Appearance::Light => self.defaults.light,
        };
        self.get(name)
            // `with_bundled` checked both names, so this only satisfies the types.
            .unwrap_or_else(|| &self.entries[0].theme)
    }

    /// The theme a `theme` setting asks for.
    ///
    /// A plain name pins that theme. An object picks `light` or `dark` by its mode,
    /// with `system_dark` (the current system appearance) deciding for `"system"`. A name
    /// that is not loaded falls back to the default theme for the appearance and adds a
    /// warning; an object that names no theme for the appearance falls back silently.
    pub fn resolve(&self, selection: &ThemeSelection, system_dark: bool) -> &Theme {
        let fallback = self.default_for(Appearance::from_dark(selection.prefers_dark(system_dark)));
        let Some(name) = selection.name(system_dark) else {
            return fallback;
        };
        if let Some(theme) = self.get(name) {
            return theme;
        }
        self.warn(ThemeWarning {
            source: "settings".to_string(),
            theme: Some(name.to_string()),
            message: format!("theme {name:?} not found; using {:?}", fallback.name),
        });
        fallback
    }

    /// Problems found while loading and resolving, oldest first, without repeats.
    pub fn warnings(&self) -> Vec<ThemeWarning> {
        self.warnings.lock().clone()
    }

    /// Returns the warnings and forgets them.
    pub fn take_warnings(&self) -> Vec<ThemeWarning> {
        std::mem::take(&mut *self.warnings.lock())
    }

    fn warn(&self, warning: ThemeWarning) {
        let mut warnings = self.warnings.lock();
        if !warnings.contains(&warning) {
            tracing::warn!(%warning, "theme warning");
            warnings.push(warning);
        }
    }
}

impl std::fmt::Debug for ThemeRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("ThemeRegistry")
            .field("themes", &self.names().collect::<Vec<_>>())
            .finish()
    }
}
