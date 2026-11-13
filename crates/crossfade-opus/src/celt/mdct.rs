//! The inverse MDCT with its windowed overlap-add (`celt/mdct.c`, `clt_mdct_backward`, float
//! build): pre-rotation, an N/4-point complex inverse FFT, post-rotation, and the TDAC
//! mirroring into the output with the low-overlap window.

use super::tables::{OVERLAP, WINDOW};
#[cfg(test)]
#[path = "mdct_tests.rs"]
mod tests;
