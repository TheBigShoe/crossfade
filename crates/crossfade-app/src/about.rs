//! What "About Crossfade" says.

/// The app's name, its version (the crates share one), what it does and its licences.
pub fn lines() -> [String; 3] {
    [
        format!("Crossfade {}", env!("CARGO_PKG_VERSION")),
        "Converts, inspects and normalises audio files, in pure Rust.".to_owned(),
        "MIT or Apache-2.0; the Opus codec is BSD-3-Clause.".to_owned(),
    ]
}
