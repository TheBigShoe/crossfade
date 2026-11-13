//! Linear prediction for the pitch-based concealment (`celt/celt_lpc.c` and `celt/pitch.c`,
//! float build): autocorrelation, Levinson-Durbin, FIR and IIR filters, and the pitch search.

#[cfg(test)]
#[path = "lpc_tests.rs"]
mod tests;
