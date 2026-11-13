//! The pyramid vector quantiser's decoding half (`celt/vq.c`): pulses to a unit vector, the
//! spreading rotation, renormalisation.

use super::ec::Ec;
#[cfg(test)]
#[path = "vq_tests.rs"]
mod tests;
