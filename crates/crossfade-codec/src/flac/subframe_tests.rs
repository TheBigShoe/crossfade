use super::Accumulator::{Narrow, Wide};
use super::*;

/// The sum of `order` products of a `bps`-bit sample and a `precision`-bit coefficient is at most
/// `order * 2^(bps + precision - 2)`; `i32` holds it only while that is below 2^31.
#[test]
fn the_accumulator_is_narrow_exactly_while_the_prediction_sum_fits_in_32_bits() {
    for (bps, precision, order, want) in [
        // The common case: 16-bit audio, 12-bit coefficients, order 8: 8 * 2^26 = 2^29.
        (16, 12, 8, Narrow),
        // Order 32 reaches 2^31, one more than i32 holds; order 31 stays below it.
        (16, 12, 31, Narrow),
        (16, 12, 32, Wide),
        (16, 15, 3, Narrow),
        (16, 15, 4, Wide),
        // 8-bit audio fits at any order with the largest precision.
        (8, 15, 32, Narrow),
        (24, 15, 1, Wide),
        (24, 7, 3, Narrow),
        (24, 7, 4, Wide),
        // A side channel is a bit wider than the stream.
        (17, 12, 15, Narrow),
        (17, 12, 16, Wide),
        (25, 5, 7, Narrow),
        (25, 5, 8, Wide),
        // The widest samples leave no room.
        (31, 1, 1, Narrow),
        (32, 1, 1, Wide),
        (33, 15, 12, Wide),
    ] {
        assert_eq!(
            lpc_accumulator(bps, precision, order),
            want,
            "bps {bps}, precision {precision}, order {order}"
        );
    }
}
