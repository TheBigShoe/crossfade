//! The PVQ codebook index (`celt/cwrs.c`): the `i`-th vector of `n` integers whose absolute
//! values sum to `k`, through the U(N, K) recurrence (the reference's generic, small-footprint
//! path, exact for every size the pulse cache allows: V(N, K) fits in 32 bits there).

use super::ec::Ec;
#[cfg(test)]
#[path = "cwrs_tests.rs"]
pub(super) mod tests;
