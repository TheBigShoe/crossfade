//! The float build's `mathops.h` and `bands.c`'s integer helpers: every transcendental through
//! `libm`, in double as the reference's macros compute them, so the output is the same bits
//! everywhere.

#[cfg(test)]
#[path = "maths_tests.rs"]
mod tests;
