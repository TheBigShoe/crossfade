//! Levels in decibels. Every conversion goes through `libm`, never the platform's `pow`, so a
//! gain is the same bits on every machine.

/// The amplitude factor of `db` decibels.
pub fn db_to_linear(db: f64) -> f64 {
    libm::pow(10.0, db / 20.0)
}
