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

/// What a preset designs: the passband edge as a fraction of the lower rate's Nyquist
/// frequency (the stopband always starts at that Nyquist frequency), the Kaiser design
/// attenuation in dB, and the table rows per sample when phases are interpolated.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Params {
    pub(crate) pass: f64,
    pub(crate) atten: f64,
    pub(crate) rows_per_sample: usize,
}

impl Quality {
    pub(crate) fn params(self) -> Params {
        match self {
            Self::Fast => Params {
                pass: 0.85,
                atten: 110.0,
                rows_per_sample: 128,
            },
            Self::Balanced => Params {
                pass: 0.91,
                atten: 130.0,
                rows_per_sample: 256,
            },
            Self::Best => Params {
                pass: 0.935,
                atten: 170.0,
                rows_per_sample: 512,
            },
        }
    }
}

impl Params {
    /// Kaiser's formula for the window shape.
    pub(crate) fn beta(&self) -> f64 {
        0.1102 * (self.atten - 8.7)
    }

    /// The -6 dB point, midway through the transition band.
    pub(crate) fn cutoff(&self) -> f64 {
        (self.pass + 1.0) / 2.0
    }

    /// Kaiser's length estimate, in samples at the lower rate.
    pub(crate) fn width(&self) -> f64 {
        (self.atten - 8.0) / (2.285 * std::f64::consts::PI * (1.0 - self.pass))
    }
}
