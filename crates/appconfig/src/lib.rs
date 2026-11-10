//! The configuration layer the desktop apps share, with no GPUI and no app-specific
//! knowledge: where the config directory lives, layered JSONC settings, a
//! comment-preserving settings and keymap editor, Zed-format keymaps and themes, and a
//! watcher that reports which kind of file changed.
//!
//! Each app supplies what is its own (names, defaults, templates, bundled themes and
//! keymaps) as arguments. This code was extracted from Serialist at commit `aff6283`.
#![forbid(unsafe_code)]

pub mod atomic;
pub mod config_watch;
pub mod keymap;
pub mod paths;
pub mod settings;
pub mod theme;

pub use atomic::write_atomic;
pub use config_watch::{ConfigEvent, ConfigWatcher, WatchSpec};
pub use keymap::{ActionRef, KeyBinding, Keymap, KeymapError, load_keymap};
pub use paths::{AppIdentity, ConfigPaths, Platform};
pub use settings::{
    EditError, FontFeatures, FontSpec, KeyChange, KeymapEditor, LineHeight, Loaded, SettingsEditor,
    SettingsError, SettingsLayer, SettingsWarning, ThemeMode, ThemeSelection, load_files,
    load_layers,
};
pub use theme::{
    Appearance, Rgba, Theme, ThemeDefaults, ThemeError, ThemeFamily, ThemeRegistry, ThemeWarning,
};

#[cfg(test)]
mod test_util;
