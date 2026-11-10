//! Themes in Zed's file format, with no GPUI types.
//!
//! A theme file is a Zed theme family (schema v0.2.0): flat dotted color keys plus
//! `players` and `syntax`. [`ThemeFamily::parse`] reads one, [`ThemeRegistry`] holds the
//! app's bundled themes plus the user's `themes/*.json`, and [`ThemeRegistry::resolve`]
//! picks the theme a `theme` setting asks for. Colors are [`Rgba`]; turning them into
//! GPUI's types is the UI's job.
//!
//! A user theme may leave keys out, so read colors with [`Theme::color_or`] and a
//! sensible fallback.

mod color;
mod family;
mod registry;

#[cfg(test)]
mod tests;

pub use color::{ColorParseError, Rgba};
pub use family::{
    Appearance, FontStyle, PlayerColors, SyntaxStyle, Theme, ThemeError, ThemeFamily, ThemeWarning,
};
pub use registry::{ThemeDefaults, ThemeRegistry};
