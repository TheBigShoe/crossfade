//! The SILK layer (RFC 6716 section 4.2; libopus `silk/`, the decoder half): the
//! linear-prediction codec used alone in SILK mode and below 8 kHz in hybrid mode. Not built
//! yet: [`SilkDecoder::decode`] returns [`Error::Unsupported`].
//!
//! # The contract with the top level
//!
//! The top level ([`crate::Decoder`], a port of `opus_decoder.c`) owns one [`SilkDecoder`]
//! and calls [`SilkDecoder::decode`] ONCE per Opus frame where `opus_decode_frame` loops over
//! `silk_Decode` (`first_frame` set on the first call): the loop belongs to this module.
//!
//! - **Range decoder.** `dec` is the frame's [`RangeDecoder`], fresh at the frame's first byte;
//!   SILK reads first, and in hybrid mode the top level then reads the redundancy flags and
//!   CELT reads on from where SILK stopped. SILK reads the frame's header (VAD flags and LBRR
//!   flags, RFC 6716 4.2.3-4.2.4) and its LBRR frames (decoded only for `fec`, otherwise read
//!   and dropped, as `silk_Decode` does), then each 10 or 20 ms SILK frame: one for a 10 or
//!   20 ms Opus frame, two for 40 ms, three for 60 ms. `None` means the frame is lost: run
//!   SILK's packet loss concealment (`lost_flag = FLAG_PACKET_LOST`).
//! - **Frame.** [`SilkFrame`]: the Opus frame's duration in samples per channel at 48 kHz
//!   (`payloadSize_ms`: 480, 960, 1920 or 2880), SILK's internal rate (`internalSampleRate`:
//!   8000 for NB, 12000 for MB, 16000 for WB and always in hybrid), the coded channels
//!   (`nChannelsInternal`: the TOC's stereo flag), and `fec` (`lost_flag =
//!   FLAG_DECODE_LBRR`: decode this packet's LBRR data in place of the previous, lost,
//!   packet). For a lost frame the rate and channels are ignored (the previous ones hold), and
//!   `frame_size` is what to conceal: the top level asks for at least 10 ms where it can, but
//!   may ask for less (a 2.5 or 5 ms CELT frame's worth after SILK): conceal 10 ms and write
//!   the first `frame_size` samples, as the reference does with its `max(F10, frame_size)`
//!   buffer. The previous packet's internal channel count, the "decode only middle" flag and
//!   the rate are SILK's own state (`nChannelsInternal`, `prev_decode_only_middle`,
//!   `fs_kHz` in `silk_decoder`), so the entry does not take them: switching between mono and
//!   stereo is handled inside, as `dec_API.c` does.
//! - **Output.** `out` has exactly `frame_size * channels` samples, interleaved, at 48 kHz:
//!   SILK resamples its 8, 12 or 16 kHz signal to 48 kHz itself, with libopus's
//!   `silk_resampler` (`silk_resampler_private_IIR_FIR` for each of these ratios) initialised
//!   for decoding, so its input delay (`delay_matrix_dec`) aligns SILK with CELT exactly as in
//!   the reference; the resampler is re-initialised when the internal rate changes
//!   (`silk_decoder_set_fs`). Mono and stereo are converted to `channels` as `silk_Decode`
//!   does (`silk_stereo_MS_to_LR`, a mono stream copied to both outputs, a stereo stream
//!   decoded to mono through the mid channel only). SILK OVERWRITES `out` with its 16-bit
//!   output scaled as the float build does: `(1.0 / 32768.0) * f32::from(sample)`; in hybrid
//!   mode CELT then adds into it.
//! - **State.** Everything in libopus's `silk_decoder` (both channels' `silk_decoder_state`,
//!   the stereo state, the resamplers) lives in [`SilkDecoder`]. [`SilkDecoder::new`] is
//!   `silk_InitDecoder`; [`SilkDecoder::reset`] is `silk_ResetDecoder`, which the top level
//!   calls on a switch from CELT to SILK or hybrid and on `OPUS_RESET_STATE`. Both clear the
//!   stereo state (`sStereo`) and `prev_decode_only_middle`, as RFC 8251 section 3 requires.
//! - **Errors.** [`Error::Corrupt`] where `silk_Decode` returns an error; for a lost frame the
//!   top level then writes silence instead ("PLC failure should not be fatal").
//!
//! # Arithmetic
//!
//! SILK's decoder is integer arithmetic in the reference, in the float build too: port it as
//! integers (`i16`/`i32`/`i64`, the `silk_SMULWB`-style macros as small inline functions in
//! `silk/macros.rs`), so that the entropy decoding, the excitation, the LPC and LTP synthesis
//! and the resampler give exactly the reference's 16-bit samples, then convert at the end.
//! Mind that Rust's debug builds trap on overflow where C wraps: the `_ovflw` macros
//! (`silk_ADD32_ovflw`, `silk_SUB32_ovflw`, `silk_MLA_ovflw`, `silk_SMLABB_ovflw`, ...) are
//! `wrapping_*`, the `_SAT` ones `saturating_*`, and every other C `+`, `-`, `*`, `<<` that can
//! overflow on hostile input must say which it is.
//!
//! RFC 8251 changes SILK in four places that MUST be applied: the stereo state reset (section
//! 3, above); the resampler buffer (section 5: `silk_resampler_private_IIR_FIR`'s buffer holds
//! `2 * RESAMPLER_MAX_BATCH_SIZE_IN + RESAMPLER_ORDER_FIR_12` 16-bit samples and copies
//! `RESAMPLER_ORDER_FIR_12` 16-bit samples, overlapping copies moved); the inverse prediction
//! gain (section 6: `LPC_inverse_pred_gain_QA` saturates `tmp_QA` and treats a 64-bit update
//! outside 32 bits as an unstable filter, returning 0); and NLSF stabilisation (section 7:
//! `silk_NLSF_stabilize` adds `NDeltaMin_Q15[i]` with `silk_ADD_SAT16`).
//!
//! # Tables
//!
//! SILK's tables (the iCDFs of RFC 6716 4.2.7, the NLSF codebooks, the LTP and pitch tables,
//! the resampler's coefficients and `delay_matrix_dec`, ...) go in `silk/tables*.rs`, private
//! to this module. Nothing else in the crate shares them; the range decoder needs none.

use crate::error::{Error, Result};
use crate::range::RangeDecoder;

/// What the top level tells SILK about one Opus frame (`opus_decode_frame`'s `DecControl` and
/// `lost_flag`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SilkFrame {
    /// Samples per channel at 48 kHz: 480, 960, 1920 or 2880 (10 to 60 ms); for a lost frame,
    /// what to conceal.
    pub(crate) frame_size: usize,
    /// SILK's internal rate in Hz: 8000, 12000 or 16000.
    pub(crate) internal_rate: u32,
    /// Channels coded in the frame (the TOC's stereo flag): 1 or 2.
    pub(crate) stream_channels: usize,
    /// Decode the frame's LBRR (forward error correction) data, standing in for the previous,
    /// lost, frame.
    pub(crate) fec: bool,
}

/// The SILK decoder's state for one stream (libopus `silk_decoder`).
#[derive(Debug, Clone)]
pub(crate) struct SilkDecoder {
    /// Output channels, 1 or 2 (`nChannelsAPI`).
    #[expect(dead_code, reason = "read once the SILK decoder is built")]
    channels: usize,
}

impl SilkDecoder {
    /// A decoder writing `channels` (1 or 2) output channels at 48 kHz (`silk_InitDecoder`).
    pub(crate) fn new(channels: usize) -> Self {
        Self { channels }
    }

    /// `silk_ResetDecoder`, with RFC 8251's stereo state reset.
    pub(crate) fn reset(&mut self) {}

    /// Decodes (`Some`) or conceals (`None`) one Opus frame's SILK part into `out`
    /// (`frame.frame_size * channels` interleaved samples at 48 kHz), per the module's
    /// contract.
    pub(crate) fn decode(
        &mut self,
        _dec: Option<&mut RangeDecoder<'_>>,
        _frame: &SilkFrame,
        _out: &mut [f32],
    ) -> Result<()> {
        Err(Error::Unsupported("silk decoding is not built yet"))
    }
}
