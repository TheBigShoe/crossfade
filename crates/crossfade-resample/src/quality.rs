//! The quality presets.

/// How hard the filter works. Every preset is linear phase; they differ in passband width,
/// stopband attenuation and so in filter length (speed). The README has the measured table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Quality {
    /// Shorter filter for previews and real-time use.
    Fast,
    /// The default: flat to 20 kHz at 44.1 kHz, images and aliases at least 110 dB down.
    #[default]
    Balanced,
    /// Wider passband and as much attenuation as f64 arithmetic usefully gives.
    Best,
}
