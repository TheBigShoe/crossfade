//! Reusable pieces of an app's settings: layered JSONC loading, and the Zed-style font
//! and theme keys an app's own settings type can embed.
//!
//! Files are JSON with comments and trailing commas. Loading layers three documents
//! and deserializes the result:
//!
//! 1. the app's bundled defaults,
//! 2. the user's `settings.json` in the config directory,
//! 3. a project-local `settings.json`.
//!
//! Objects merge key by key, arrays and scalars replace, and `null` unsets an optional
//! key, so a user file may hold a single key. Unknown keys are ignored and reported as
//! [`SettingsWarning`]s; a wrong type or bad syntax is a [`SettingsError`] naming the
//! file, line and column.
//!
//! [`SettingsEditor`] and [`KeymapEditor`] change one key of a file and leave the rest
//! of its text as the user wrote it.

pub mod de;
mod edit;
#[cfg(test)]
mod fixture;
mod font;
mod keymap_edit;
mod load;
mod theme_selection;

pub use edit::{EditError, KeyChange, SettingsEditor};
pub use font::{FontFeatures, FontSpec, LineHeight};
pub use keymap_edit::KeymapEditor;
pub use load::{Loaded, SettingsError, SettingsLayer, SettingsWarning, load_files, load_layers};
pub use theme_selection::{ThemeMode, ThemeSelection};
