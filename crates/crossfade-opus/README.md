# crossfade-opus

Opus (RFC 6716, updated by RFC 8251) in pure Rust, depending only on `libm`:

- **Decoding**: the range decoder (RFC 6716 section 4.1), the packet parser (section 3 and
  the self-delimiting framing of Appendix B), SILK, CELT and hybrid frames with their
  transitions, concealment and FEC, multistream (families 0, 1 and 255), and the Ogg
  identification and comment headers as data (RFC 7845 section 5). All twelve RFC 8251 test
  vectors conform.
- **Encoding**, CELT-only for now: `Encoder` turns 48 kHz PCM (mono or stereo, full scale
  1.0) into standard Opus packets of one 2.5, 5, 10 or 20 ms frame (code 0), CBR, VBR or
  constrained VBR, from 500 b/s to 300 kb/s per channel, with `lookahead()` (312 samples,
  as libopus's) for the container's pre-skip. Every packet decodes, here and in libopus, with
  the encoder's final range.

## The encoder

A port of libopus 1.5.2's float encoder at complexity 9 (its default) without the tonality
analysis (`analysis.c`), whose "analysis not valid" paths it takes:

- the Opus layer (`opus_encoder.c`): the 3 Hz DC filter and the 4 ms delay of
  `OPUS_APPLICATION_AUDIO`, the rate-dependent stereo/mono and bandwidth decisions with their
  hysteresis, the narrowing of the stereo image at low rates, CBR packet sizes, the TOC
  byte, "PLC frames" when the rate cannot fund a frame (below 9.6 kb/s at 2.5 ms), and a
  frame of NaN, infinite or absurd samples coded as silence;
- CELT (`celt_encoder.c` and what it calls): pre-emphasis, the pitch pre-filter
  (`run_prefilter`, `pitch.c`), transient analysis and short blocks (with the second MDCT
  and the transient patch), the forward MDCT, band energies, coarse energy (intra and inter,
  two-pass), fine energy and its final bits, `tf_analysis`, the spreading decision, dynamic
  allocation, the allocation trim, dual stereo and intensity decisions, the shared allocator,
  the PVQ search, the band shapes with the theta search (RDO) and folding, the anti-collapse
  flag, and CBR, VBR and constrained VBR rate control.

Not in it: SILK and hybrid modes, multistream encoding, DTX, LFE, surround masking, the
tonality analysis, custom modes, fixed point.

Measured on a second of a music-like signal (`stereo_quality_rises_with_the_bitrate`), 20 ms
stereo frames, decoded by this crate, SNR against the DC-filtered input and the
`opus_compare` figure Q (a strict figure made for comparing decoders: negative for any lossy
encoding at these rates):

| Bitrate | CBR SNR | CBR Q | VBR SNR | VBR Q |
|---|---|---|---|---|
| 32 kb/s | 15.0 dB | -706 | 16.0 dB | -82 |
| 64 kb/s | 18.3 dB | -66 | 19.6 dB | -56 |
| 96 kb/s | 21.2 dB | -50 | 22.9 dB | -31 |
| 128 kb/s | 23.8 dB | -53 | 24.7 dB | -31 |
| 256 kb/s | 27.7 dB | -31 | 28.2 dB | 55 |

Against libopus 1.4 forced to CELT-only at complexity 9 (a development check with the
system's libopus, not committed), on four seconds of the same signal: in CBR the two are
within 0.5 dB of SNR and close in Q at every rate and frame size; in VBR libopus spends 10
to 25% more bits on this signal (its tonality analysis raises the target), for 1 to 1.5 dB
more SNR. Constrained VBR holds the bitrate plus one TOC byte per packet, exactly as libopus
does; unconstrained VBR, open-loop as in libopus, came within -11% to +5% of the target over
20 s of music and noise. The encoder runs at about 67 times real time for 128 kb/s stereo in
release builds on one core here (libopus with its SIMD: about 110).

The packets are the same bits on every platform (`libm` for every transcendental, no
`mul_add`); `the_same_input_gives_the_same_bytes_on_every_platform` freezes a digest of them.

## Licence

This crate is a port of the reference decoder of RFC 6716 and of libopus
(<https://github.com/xiph/opus>), and is therefore licensed, unlike the rest of the Crossfade
workspace, under the 3-clause BSD licence of those sources: see `COPYING`, which also points to
the royalty-free patent licences that cover Opus. `src/testing/compare.rs` is a port of
`opus_compare.c` and keeps its own 2-clause BSD notice.
