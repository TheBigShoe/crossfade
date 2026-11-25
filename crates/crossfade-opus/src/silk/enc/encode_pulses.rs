//! The excitation's pulses, written (libopus `silk/encode_pulses.c`, the encode halves of
//! `shell_coder.c` and `code_signs.c`; RFC 6716 4.2.7.8).

use crate::range_enc::RangeEncoder;
use crate::silk::state::MAX_FRAME_LENGTH;

/// `silk_encode_pulses`: `pulses` holds the frame's signed pulses; a 10 ms frame at 12 kHz has
/// its last, partial, block zeroed and coded whole.
pub(super) fn encode_pulses(
    _enc: &mut RangeEncoder,
    _signal_type: i32,
    _quant_offset_type: i32,
    _pulses: &mut [i8; MAX_FRAME_LENGTH],
    _frame_length: usize,
) {
}

#[cfg(test)]
#[path = "encode_pulses_tests.rs"]
mod tests;
