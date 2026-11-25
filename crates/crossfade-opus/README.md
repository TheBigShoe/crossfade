# crossfade-opus

Opus (RFC 6716, updated by RFC 8251) in pure Rust, depending only on `libm`:

- **Decoding**: the range decoder (RFC 6716 section 4.1), the packet parser (section 3 and
  the self-delimiting framing of Appendix B), SILK, CELT and hybrid frames with their
  transitions, concealment and FEC, multistream (families 0, 1 and 255), and the Ogg
  identification and comment headers as data (RFC 7845 section 5). All twelve RFC 8251 test
  vectors conform.
- **Encoding**: `Encoder` turns 48 kHz PCM (mono or stereo, full scale 1.0) into standard
  Opus packets of 2.5 to 60 ms in SILK-only, hybrid or CELT-only mode, chosen by the rate
  and libopus's speech/music analysis (or a signal hint) as libopus chooses, with the 5 ms
  redundant frames of mode switches, CBR, VBR or constrained VBR, in-band FEC and DTX;
  `MultistreamEncoder` codes 1 to 8 channels (mapping families 0 and 1, the Vorbis order)
  with the surround encoder's rate allocation and masking and an LFE stream in CELT's LFE
  mode. `lookahead()` (312 samples, as libopus's) is the pre-skip; `head()` gives the
  `OpusHead` (`to_bytes()` for an Ogg writer). Every packet decodes, here and in libopus,
  with the encoder's final range.

## The encoder

A port of libopus 1.5.2's float encoder at complexity 9 (its default), its tonality analysis
included, over a port of its fixed-point SILK encoder (so SILK is integer arithmetic, CELT
and the analysis float through `libm`: a pairing libopus never builds, so SILK's bytes are not
libopus's, though every decoder reads them):

- the tonality and speech/music analysis (`analysis.c`, `mlp.c` and its network's weights,
  `mlp_data.c`): each 20 ms of the downmix at 24 kHz through a windowed FFT, the tonality from
  the phase's predictability per bin, band energies, the activity, the leakage boosts, the
  bandwidth, and a small GRU network's probabilities of music and of activity, read with
  `tonality_get_info`'s look-ahead and switching thresholds. As at libopus's complexity 7 and
  up it always runs; its outputs are libopus's within 6e-6 on the test signals (a pure tone's
  within 0.04, its quiet bins being FFT rounding) and within 1.5e-4 on music;
- the Opus layer (`opus_encoder.c`, `OPUS_APPLICATION_AUDIO` and `VOIP`): the 3 Hz DC filter
  (VoIP: the high-pass that follows SILK's pitch smoother), the 4 ms delay, the voice
  estimate (`Signal::Auto` from the analysis's music probability, at most 90% voice for
  audio; `Voice` and `Music` are `OPUS_SET_SIGNAL`), the SILK/CELT threshold by rate and
  stereo width with its hysteresis, mono/stereo, the bandwidth from narrowband to fullband
  (SILK widening only when its switch allows; the detected bandwidth at low rates), SILK's
  share of a hybrid frame and the high band's gain, the narrowing of the stereo image at low
  rates, the 5 ms redundant CELT frames and prefills of every switch (to and from CELT, and
  SILK's bandwidth switches), in-band FEC (`decide_fec`) and DTX (the analysis's activity
  and digital silence as TOC bytes, SILK's own without the analysis), CBR in every mode
  (code 3 padding), 40 and 60 ms packets (one SILK frame, or 20 ms CELT and hybrid frames
  through a port of the repacketizer, each frame reading its own analysis), "PLC frames"
  when the rate cannot fund a frame, a forced mode and bandwidth (`OPUS_SET_FORCE_MODE`,
  `OPUS_SET_BANDWIDTH`), a frame of NaN, infinite or absurd samples coded as silence, and
  the input's depth (`EncoderConfig::lsb_depth`, `OPUS_SET_LSB_DEPTH`: 8 to 24 bits, 24 by
  default, anything else refused), which sets what is digital silence (here, in the
  analysis and in CELT), the analysis's noise floor for the bandwidth and CELT's for its
  band boosts and VBR cap;
- SILK (`silk/enc_API.c` and everything under it, `silk/fixed/` for the analysis): see
  `src/silk/enc/mod.rs`; byte-identical to libopus's fixed-point build where tested;
- CELT (`celt_encoder.c` and what it calls): pre-emphasis, the pitch pre-filter, transient
  analysis and short blocks (and hybrid's weak transients), the MDCT, band energies, coarse
  and fine energy, `tf_analysis`, spreading, dynamic allocation, the trim, stereo decisions,
  the allocator, the PVQ search and band shapes, anti-collapse, the hybrid half (from band
  17, SILK's offset in the VBR target), the LFE mode, and CBR, VBR and constrained VBR; with
  the analysis's pitch ratio, leakage boosts, tonality slope (trim), activity, tonality and
  pitch change (VBR target) and bandwidth (allocation), and the surround masks;
- multistream (`opus_multistream_encoder.c`): the Vorbis layouts, `surround_rate_allocation`,
  the bandwidth set from the total rate, coupled streams forced to stereo CELT, the LFE, the
  surround masking (`surround_analysis`: each channel's bands against what the others mask,
  handed to its stream, where CELT moves its bits, dynalloc, trim and VBR target by them and
  SILK its VBR rate), the self-delimited framing.

Two deliberate deviations: a packet of several 20 ms frames prefills SILK once, and a
stereo-to-mono switch inside one downmixes in its last frame (libopus prefills before every
frame, and sets `force_channels` for good). Two of libopus's quirks are kept: a frame with a NaN or
infinite sample poisons the analysis's resampler for good (libopus's does the same; a reset
clears it), and a hybrid frame whose SILK takes all but 500 b/s of the rate (with surround
masks) codes CELT at the frame's maximum, 1275 bytes. Not in it: DRED and the deep-learning
features, 80 to 120 ms packets, ambisonics and family 255, custom modes, fixed-point CELT,
complexity settings (always 9), `OPUS_APPLICATION_RESTRICTED_LOWDELAY`.

What each rate becomes on the test speech, 20 ms VBR
(`the_mode_and_bandwidth_follow_the_bitrate_and_the_signal`; Auto takes it for speech, as
libopus does, and music for CELT from about 16 kb/s mono:
`auto_codes_music_as_celt_and_speech_as_silk_or_hybrid`):

| | 6 kb/s | 9 | 12 | 16 | 24 | 32 | 48 | 64 | 96 |
|---|---|---|---|---|---|---|---|---|---|
| mono, Auto | SILK NB | SILK NB | SILK WB | hybrid FB | hybrid FB | | | CELT FB | |
| mono, Voice | | | SILK WB | | hybrid FB | | hybrid FB | | CELT FB |
| mono, Music | | | CELT SWB | | | | | | |
| stereo, Auto | | | SILK WB | hybrid FB | | hybrid FB | | | |
| stereo, Voice | | | | | | hybrid FB | | CELT FB | |

### Quality and rate

Music (a second of a music-like signal, 20 ms stereo, Auto,
`stereo_quality_rises_with_the_bitrate`), SNR against the DC-filtered input and the
`opus_compare` figure Q (a strict figure made for comparing decoders: negative for any lossy
encoding at these rates); in VBR the analysis spends 10 to 34% over the bitrate on this
tonal music, as libopus does:

| Bitrate | CBR SNR | CBR Q | VBR SNR | VBR Q | VBR rate |
|---|---|---|---|---|---|
| 32 kb/s | 12.5 dB | -650 | 14.0 dB | -564 | 35.2 kb/s |
| 64 kb/s | 18.7 dB | -69 | 21.2 dB | -61 | 85.5 kb/s |
| 96 kb/s | 21.5 dB | -50 | 24.1 dB | -32 | 122.3 kb/s |
| 128 kb/s | 24.2 dB | -79 | 25.8 dB | -43 | 161.0 kb/s |
| 256 kb/s | 28.1 dB | -40 | 29.0 dB | 60 | 297.8 kb/s |

(At 32 kb/s the analysis takes the first frames for speech and codes them in hybrid, as
libopus does.) Speech (a synthetic voice with pauses, mono, `Signal::Voice`,
`speech_quality_at_low_rates`), segmental SNR: 8.95 dB at 8 kb/s (SILK NB), 8.32 at 12 (SILK
WB), 8.42, 8.69 and 8.80 at 16, 24 and 32 kb/s (hybrid FB). A mode switch on a steady tone
leaves no click: the decoded second difference near the switch is 2.1 to 2.3 times a steady
encode's into SILK or hybrid and 0.9 times back into CELT (10 times without the redundant
frame). 5.1 music-like surround: 21.8, 28.3 and 30.6 dB mean channel SNR at 128, 256 and 448
kb/s.

CBR packets are the size the bitrate gives in every mode and for multistream totals.
Constrained VBR holds the bitrate plus one TOC byte per packet in CELT, as libopus does;
unconstrained VBR is open-loop as in libopus, the analysis raising the target for tonal
frames and lowering it for inactive ones: +33% on the tonal test music at 64 kb/s, -9% on
noise at 128 kb/s, +4% to +58% for the test surround from 64 to 448 kb/s.

### Against libopus

A development check, not committed: libopus 1.5.2's float encoder built from source (plain C,
no intrinsics, `-ffp-contract=off`), complexity 9, against this crate on the same 48 kHz input
(five music files of the FLAC conformance corpus resampled, two synthetic voices, the corpus's
5.1 and 7.1 files and a 5.1 upmix of music), each output decoded by this crate:

| | runs | rate against libopus | SNR against libopus | packets byte for byte | mode and bandwidth |
|---|---|---|---|---|---|
| stereo music, 64 to 256 kb/s, VBR and constrained VBR | 40 | +0.0% (all within 0.05%) | +0.00 dB | 68 to 100% (mean 89%) | the same in every frame |
| Auto, 12 to 48 kb/s, mono and stereo, music and speech | 40 | -0.1% to +3.0% | -0.12 to +0.13 dB | 0 to 98% (SILK: 0 to 4%) | the same in every frame |
| 5.1 and 7.1, 128 to 448 kb/s, VBR and CBR | 18 | -0.1% to +0.0% | -0.01 to +0.02 dB | 5 to 97% | the same in every frame |
| 2.5 to 60 ms frames, VoIP, CBR, constrained VBR | 13 | 0.0% to +1.8% | -0.02 to +0.03 dB | 1 to 100% | the same in every frame |

CELT frames are libopus's byte for byte unless the last bit of an FFT, MDCT or `libm` call
differs (ours are libopus 1.1's transforms); SILK and hybrid frames are not (fixed-point SILK
against libopus's float SILK), whence their 1 to 3% (more on noise taken for speech: +8% at
10 ms in hybrid). The analysis's outputs on music are within 1.5e-4 of libopus's, its voice
estimate and validity the same in every frame, and the surround masks within 5e-5. Before
the analysis our VBR rates on these music files were 15% under to 8% over libopus's (up to
1.3 dB less SNR), Auto chose another mode in up to all frames of a run, and surround VBR
was 37% under to 42% over. At an input depth of 16 bits on both sides (the corpus music, the
synthetic voices, tones under hiss near the 16-bit noise floor, music at -60 dB, a tone under
one 16-bit step), 68 runs from 12 to 128 kb/s choose the same mode and bandwidth in every
frame, and CELT's rates are within 0.04% of libopus's (SILK's differ as at 24 bits); 16 bits
narrows hissy signals to superwideband at 64 kb/s, codes -60 dB music up to 24% smaller at
128 kb/s, and a signal under one 16-bit step as silence, as in libopus. `opusdec` (libopus 1.4) decodes every packet of ours (every
mode, frame size, bitrate sweep across the modes, FEC, 3 to 8 channels) to this crate's
output within 2e-5, and `opusinfo` reports no problem. The encoder runs at about 76 times
real time for 128 kb/s stereo in release builds on one core here (87 before the analysis,
which takes about 0.9 ms a second of audio; the rest is the bits it adds).

The packets are the same bits on every platform (`libm` for every transcendental, no
`mul_add`, SILK in integers); frozen digests cover CELT, SILK, hybrid, 40/60 ms, FEC and 5.1.

## Licence

This crate is a port of the reference decoder of RFC 6716 and of libopus
(<https://github.com/xiph/opus>), and is therefore licensed, unlike the rest of the Crossfade
workspace, under the 3-clause BSD licence of those sources: see `COPYING`, which also points to
the royalty-free patent licences that cover Opus. `src/testing/compare.rs` is a port of
`opus_compare.c` and keeps its own 2-clause BSD notice.
