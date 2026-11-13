//! The CELT layer (RFC 6716 section 4.3; libopus `celt/`): the MDCT codec used alone in CELT
//! mode and above 8 kHz in hybrid mode. Not built yet: [`CeltDecoder::decode`] returns
//! [`Error::Unsupported`].
//!
//! # The contract with the top level
//!
//! The top level ([`crate::Decoder`], a port of `opus_decoder.c`) owns one [`CeltDecoder`]
//! and calls it the way `opus_decode_frame` calls `celt_decode_with_ec`:
//!
//! - **Range decoder.** `dec` is the frame's [`RangeDecoder`], shared with SILK in hybrid
//!   mode: CELT starts reading where SILK and the top level's redundancy flags stopped, and its
//!   [`RangeDecoder::storage`] is CELT's `len` (the top level has already taken any
//!   redundancy bytes off the end with [`RangeDecoder::shrink`]). CELT reads raw bits from the
//!   end with [`RangeDecoder::dec_bits`] and budgets with `tell`/`tell_frac` exactly as
//!   `celt_decoder.c` does; the final range is what `dec.rng()` holds when CELT returns (the
//!   top level reads it). `None` means the frame is lost: conceal it (`celt_decode_lost`).
//!   The top level passes `None` whenever `celt_decode_with_ec` would see `data == NULL ||
//!   len <= 1`; CELT never sees a frame of fewer than 2 bytes.
//! - **Frame.** [`CeltFrame`]: the frame size in samples per channel at 48 kHz (120, 240, 480
//!   or 960: CELT always runs at 48 kHz here, `downsample` is 1), the start band (0, or 17 in
//!   hybrid: `CELT_SET_START_BAND`), the end band from the bandwidth (13 NB, 17 MB and WB, 19
//!   SWB, 21 FB: `CELT_SET_END_BAND`, from [`crate::Bandwidth::celt_end_band`]), the coded
//!   channels (`CELT_SET_CHANNELS(stream_channels)`: the TOC's stereo flag), and whether to
//!   accumulate. For a lost frame the top level passes the bands and channels of the last
//!   frame it set (libopus keeps them in the CELT state, which a PLC call does not change).
//! - **Output.** `out` has exactly `frame_size * channels` samples, interleaved, at 48 kHz, in
//!   the float build's scale (full scale 1.0: `celt_decode_with_ec`'s `pcm` for
//!   `opus_decode_float`). With `accumulate` false CELT overwrites them; with it true it ADDS
//!   its output to what is there (the top level's SILK output in hybrid mode: float addition
//!   commutes, so this equals the reference's `pcm[i] + (1/32768) * pcm_silk[i]`). A mono
//!   stream decoded to two channels is duplicated, a stereo stream to one channel is mixed, as
//!   `celt_decoder.c` does (`C`, `CC`).
//! - **State.** Everything `celt_decoder.c` keeps between calls lives in [`CeltDecoder`]:
//!   overlap and decode memory, old band energies and their history, the PLC state, the
//!   post-filter (pitch, gain, tapset, old ones), the de-emphasis memory, the RNG, the last
//!   start/end band and coded channels. [`CeltDecoder::reset`] is `OPUS_RESET_STATE`.
//! - **Errors.** [`Error::Corrupt`] where `celt_decode_with_ec` returns `OPUS_INTERNAL_ERROR`
//!   (with `out` left in any state: the top level truncates). The range decoder's error flag
//!   is not an error by itself: the reference carries on.
//!
//! # Arithmetic
//!
//! Port the FLOAT build of libopus (`opus_val16`/`opus_val32`/`celt_sig` are `f32`, `celt_norm`
//! `f32`) as the RFC's reference decoder has it (without `FLOAT_APPROX`): `celt_exp2(x)` is
//! `exp(0.6931471805599453094 * x)` in double, `celt_log2(x)` `1.442695040888963387 * log(x)`,
//! `celt_sqrt` `sqrt`, `celt_cos_norm(x)` `cos(0.5 * PI * x)`, every transcendental through
//! `libm` and no `mul_add`, so the output is the same bits everywhere. Everything that decides
//! what is READ (bit allocation in `rate.c`, `tf`, spreading, intensity and dual stereo,
//! `cwrs.c`'s PVQ indexing, `laplace.c`, the coarse/fine/final energy symbol decoding, the
//! anti-collapse and post-filter flags) is integer arithmetic in the reference and must stay
//! integer here: the per-packet final range check of the conformance tier
//! (`crossfade-format/tests/conformance_opus.rs`) proves it exact.
//!
//! RFC 8251 changes CELT in two places that MUST be applied: the cap on band energy (section
//! 8: the log energy is clamped to at most 32 before `celt_exp2` turns it into a linear band
//! amplitude; `log2Amp` in the RFC's `quant_bands.c`, `denormalise_bands` in current libopus),
//! and hybrid folding (section 9, `bands.c`'s `quant_all_bands`: `lowband_offset` is also set
//! when `i == start + 1`, the first band's folding data is duplicated for the second, and the
//! `fold_end` loop stops at `i`). Section 10 allows not applying the
//! 180-degree intensity-stereo phase inversion: the `m.dec` references; passing either set
//! is conformant, so apply it (the default, `OPUS_SET_PHASE_INVERSION_DISABLED(0)`).
//!
//! # Tables
//!
//! CELT's tables (the 48 kHz mode: `eBands`, `eband5ms`, the allocation table `band_allocation`,
//! `e_prob_model`, `cache_index50`/`cache_bits50`/`cache_caps50`, the windows and MDCT twiddles,
//! `trim_icdf`, `spread_icdf`, `tapset_icdf`, `tf_select_table`, `pred_coef`/`beta_coef`, ...)
//! go in `celt/tables.rs`, private to this module. Nothing else in the crate shares them;
//! the range decoder needs none.

use crate::error::{Error, Result};
use crate::range::RangeDecoder;

/// What the top level tells CELT about one frame (the `celt_decoder_ctl`s `opus_decode_frame`
/// issues before `celt_decode_with_ec`, and its arguments).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CeltFrame {
    /// Samples per channel at 48 kHz: 120, 240, 480 or 960 (2.5 to 20 ms).
    pub(crate) frame_size: usize,
    /// The first band coded: 0, or 17 in hybrid mode (SILK has the band below 8 kHz).
    pub(crate) start_band: usize,
    /// One past the last band coded: 13, 17, 19 or 21.
    pub(crate) end_band: usize,
    /// Channels coded in the frame (the TOC's stereo flag): 1 or 2.
    pub(crate) stream_channels: usize,
    /// Add into `out` (hybrid: SILK wrote it first) instead of overwriting it.
    pub(crate) accumulate: bool,
}

/// The CELT decoder's state for one stream (libopus `CELTDecoder`).
#[derive(Debug, Clone)]
pub(crate) struct CeltDecoder {
    /// Output channels, 1 or 2 (`CC`).
    #[expect(dead_code, reason = "read once the CELT decoder is built")]
    channels: usize,
}

impl CeltDecoder {
    /// A decoder writing `channels` (1 or 2) output channels (`celt_decoder_init` at 48 kHz).
    pub(crate) fn new(channels: usize) -> Self {
        Self { channels }
    }

    /// Forgets the stream, as `OPUS_RESET_STATE` does.
    pub(crate) fn reset(&mut self) {}

    /// Decodes (`Some`) or conceals (`None`) one frame into `out` (`frame.frame_size *
    /// channels` interleaved samples), per the module's contract (`celt_decode_with_ec`).
    pub(crate) fn decode(
        &mut self,
        _dec: Option<&mut RangeDecoder<'_>>,
        _frame: &CeltFrame,
        _out: &mut [f32],
    ) -> Result<()> {
        Err(Error::Unsupported("celt decoding is not built yet"))
    }
}
