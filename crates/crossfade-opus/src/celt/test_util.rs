//! Test helpers shared by the CELT modules.

use crate::testing::range_encoder::{Op, encode_into};

/// `ops` range-coded into a frame of the largest CELT size (1275 bytes). The testing module's
/// `encode_ops` sizes its frame from `bytes_needed`, which does not count the 0xFF bytes held
/// back for a carry, so long runs of improbable symbols do not fit its estimate.
pub(super) fn encode(ops: &[Op]) -> Vec<u8> {
    encode_into(ops, 1275).expect("fits in 1275 bytes").1
}
