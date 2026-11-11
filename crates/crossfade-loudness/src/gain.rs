//! The gain that brings a programme to a target loudness without its true peak passing a
//! ceiling. Deterministic: `libm` only.

/// Result of [`normalisation_gain`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gain {
    /// Factor to multiply every sample by.
    pub linear: f64,
    /// The ceiling, not the target, set the gain.
    pub limited_by_true_peak: bool,
}

/// Largest correction asked for, in dB: keeps the factor finite for absurd inputs.
const MAX_DB: f64 = 200.0;

/// Gain to take `integrated_lufs` to `target_lufs`, reduced if needed so a true peak of
/// `true_peak` (linear) stays at or below `ceiling_dbtp`.
///
/// No loudness (`None`, NaN or infinite) or a NaN target gives unity: there is nothing to
/// normalise. A zero true peak, or a NaN ceiling, imposes no limit.
pub fn normalisation_gain(
    integrated_lufs: Option<f64>,
    target_lufs: f64,
    true_peak: f64,
    ceiling_dbtp: f64,
) -> Gain {
    let unity = Gain {
        linear: 1.0,
        limited_by_true_peak: false,
    };
    let Some(loudness) = integrated_lufs.filter(|l| l.is_finite()) else {
        return unity;
    };
    if target_lufs.is_nan() {
        return unity;
    }
    let delta = (target_lufs - loudness).clamp(-MAX_DB, MAX_DB);
    let mut linear = libm::pow(10.0, delta / 20.0);
    let mut limited = false;
    if true_peak > 0.0 && !ceiling_dbtp.is_nan() {
        let room = libm::pow(10.0, ceiling_dbtp.min(MAX_DB) / 20.0) / true_peak;
        if linear > room {
            linear = room;
            limited = true;
        }
    }
    Gain {
        linear,
        limited_by_true_peak: limited,
    }
}
