//! Test helpers shared by the CELT modules.

use crate::testing::range_encoder::{Op, encode_ops};

/// `ops` range-coded into the smallest frame that holds them.
pub(super) fn encode(ops: &[Op]) -> Vec<u8> {
    encode_ops(ops).1
}
