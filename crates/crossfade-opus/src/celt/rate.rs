//! Bit allocation (`celt/rate.c`, `rate.h`): which bands get how many bits, in exact integer
//! arithmetic, because it decides what is read next.

use super::ec::Ec;
use super::tables::NB_EBANDS;
#[cfg(test)]
#[path = "rate_tests.rs"]
mod tests;
