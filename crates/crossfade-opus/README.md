# crossfade-opus

Opus (RFC 6716, updated by RFC 8251) in pure Rust, depending only on `libm`:

- **Decoding**: the range decoder (RFC 6716 section 4.1), the packet parser (section 3 and
  the self-delimiting framing of Appendix B), SILK, CELT and hybrid frames with their
  transitions, concealment and FEC, multistream (families 0, 1 and 255), and the Ogg
  identification and comment headers as data (RFC 7845 section 5). All twelve RFC 8251 test
  vectors conform.
- **Encoding**: `Encoder` turns 48 kHz PCM (mono or stereo, full scale 1.0) into standard
  Opus packets of 2.5 to 60 ms in SILK-only, hybrid or CELT-only mode, chosen by the rate
  and a signal hint as libopus chooses, with the 5 ms redundant frames of mode switches, CBR,
  VBR or constrained VBR, in-band FEC and DTX; `MultistreamEncoder` codes 1 to 8 channels
  (mapping families 0 and 1, the Vorbis order) with the surround encoder's rate allocation
  and an LFE stream in CELT's LFE mode. `lookahead()` (312 samples, as libopus's) is the
  pre-skip; `head()` gives the `OpusHead` (`to_bytes()` for an Ogg writer). Every packet
  decodes, here and in libopus, with the encoder's final range.

## The encoder

A port of libopus 1.5.2's float encoder at complexity 9 (its default) without the tonality
analysis (`analysis.c`), whose "analysis not valid" paths it takes, over a port of its
fixed-point SILK encoder (so SILK is integer arithmetic, CELT float through `libm`: a pairing
libopus never builds, so the bytes are not libopus's, though every decoder reads them):

- the Opus layer (`opus_encoder.c`, `OPUS_APPLICATION_AUDIO` and `VOIP`): the 3 Hz DC filter
  (VoIP: the high-pass that follows SILK's pitch smoother), the 4 ms delay, the voice
  estimate (`Signal::Auto` assumes a little voice, as libopus does without its analysis;
  `Voice` and `Music` are `OPUS_SET_SIGNAL`), the SILK/CELT threshold by rate and stereo
  width with its hysteresis, mono/stereo, the bandwidth from narrowband to fullband (SILK
  widening only when its switch allows), SILK's share of a hybrid frame and the high band's
  gain, the narrowing of the stereo image at low rates, the 5 ms redundant CELT frames and
  prefills of every switch (to and from CELT, and SILK's bandwidth switches), in-band FEC
  (`decide_fec`) and DTX (SILK's, and digital silence as TOC bytes), CBR in every mode
  (code 3 padding), 40 and 60 ms packets (one SILK frame, or 20 ms CELT and hybrid frames
  through a port of the repacketizer), "PLC frames" when the rate cannot fund a frame, a
  forced mode and bandwidth (`OPUS_SET_FORCE_MODE`, `OPUS_SET_BANDWIDTH`), and a frame of
  NaN, infinite or absurd samples coded as silence;
- SILK (`silk/enc_API.c` and everything under it, `silk/fixed/` for the analysis): see
  `src/silk/enc/mod.rs`; byte-identical to libopus's fixed-point build where tested;
- CELT (`celt_encoder.c` and what it calls): pre-emphasis, the pitch pre-filter, transient
  analysis and short blocks (and hybrid's weak transients), the MDCT, band energies, coarse
  and fine energy, `tf_analysis`, spreading, dynamic allocation, the trim, stereo decisions,
  the allocator, the PVQ search and band shapes, anti-collapse, the hybrid half (from band
  17, SILK's offset in the VBR target), the LFE mode, and CBR, VBR and constrained VBR;
- multistream (`opus_multistream_encoder.c`): the Vorbis layouts, `surround_rate_allocation`,
  the bandwidth set from the total rate, coupled streams forced to stereo CELT, the LFE, the
  self-delimited framing.

Two deliberate deviations: a packet of several 20 ms frames prefills SILK once, and a
stereo-to-mono switch inside one downmixes in its last frame (libopus prefills before every
frame, and sets `force_channels` for good). Not in it: the tonality analysis (so no
speech/music or bandwidth detection: give a `Signal` hint), surround masking
(`surround_analysis` and the energy masks), DRED and the deep-learning features, 80 to 120 ms
packets, ambisonics and family 255, custom modes, fixed-point CELT.

What each rate becomes, 20 ms VBR (`the_mode_and_bandwidth_follow_the_bitrate_and_the_signal`):

| | 6 kb/s | 9 | 12 | 16 | 24 | 32 | 48 | 64 | 96 |
|---|---|---|---|---|---|---|---|---|---|
| mono, Auto | SILK NB | SILK NB | hybrid SWB | hybrid FB | CELT FB | CELT FB | CELT | CELT | CELT |
| mono, Voice | | | SILK WB | | hybrid FB | | hybrid FB | | CELT FB |
| mono, Music | | | CELT SWB | | | | | | |
| stereo, Auto | | | hybrid SWB | hybrid FB | | CELT FB | | | |
| stereo, Voice | | | | | | hybrid FB | | CELT FB | |

### Quality and rate

Music (a second of a music-like signal, 20 ms stereo CELT, `stereo_quality_rises_with_the_bitrate`),
SNR against the DC-filtered input and the `opus_compare` figure Q (a strict figure made for
comparing decoders: negative for any lossy encoding at these rates):

| Bitrate | CBR SNR | CBR Q | VBR SNR | VBR Q |
|---|---|---|---|---|
| 32 kb/s | 15.0 dB | -706 | 16.0 dB | -82 |
| 64 kb/s | 18.3 dB | -66 | 19.6 dB | -56 |
| 96 kb/s | 21.2 dB | -50 | 22.9 dB | -31 |
| 128 kb/s | 23.8 dB | -53 | 24.7 dB | -31 |
| 256 kb/s | 27.7 dB | -31 | 28.2 dB | 55 |

Speech (a synthetic voice with pauses, mono, `Signal::Voice`, `speech_quality_at_low_rates`),
segmental SNR: 8.95 dB at 8 kb/s (SILK NB), 8.40 at 12 (SILK WB), 8.43, 8.68 and 8.85 at 16,
24 and 32 kb/s (hybrid FB). A mode switch on a steady tone leaves no click: the decoded
second difference near the switch is 2.1 to 2.3 times a steady encode's into SILK or
hybrid and 0.9 times back into CELT (10 times without the redundant frame). 5.1 music-like
surround: 23.8, 27.2 and 29.9 dB mean channel SNR at 128, 256 and 448 kb/s.

CBR packets are the size the bitrate gives in every mode and for multistream totals.
Constrained VBR holds the bitrate plus one TOC byte per packet, as libopus does; unconstrained
VBR is open-loop as in libopus: -11% to +5% on music and noise in CELT, within 25% on speech
in SILK and hybrid (SILK spends less in pauses), -7% to +33% for surround at the lowest rate
tried (5.1 at 64 kb/s), +2% to +15% from 192 kb/s.

Against libopus 1.4 (a development check with the system's `opusenc` at complexity 9, decoded
by `opusdec`, not committed), over 6 s of the same signals: `opusdec` decodes every packet of
ours (every mode, frame size, bitrate sweep across the modes, FEC, 3 to 8 channels) to this
crate's output within 2e-5, and `opusinfo` reports no problem. On speech with `--speech`, the
two agree within 0.1 dB of segmental SNR (0.4 dB in CBR at 16 kb/s) at the same rate (9.50
against 9.52 kb/s of payload at 12 kb/s). On music libopus's analysis raises the VBR target
(10 to 25% more bits, 1 to 2 dB more SNR); for surround its VBR spends 37 to 48% more (its
surround masking and analysis), and in CBR at 256 kb/s 5.1 the two are 0.4 dB apart. The
encoder runs at about 67 times real time for 128 kb/s stereo CELT in release builds on one
core here (libopus with its SIMD: about 110).

The packets are the same bits on every platform (`libm` for every transcendental, no
`mul_add`, SILK in integers); frozen digests cover CELT, SILK, hybrid, 40/60 ms, FEC and 5.1.

## Licence

This crate is a port of the reference decoder of RFC 6716 and of libopus
(<https://github.com/xiph/opus>), and is therefore licensed, unlike the rest of the Crossfade
workspace, under the 3-clause BSD licence of those sources: see `COPYING`, which also points to
the royalty-free patent licences that cover Opus. `src/testing/compare.rs` is a port of
`opus_compare.c` and keeps its own 2-clause BSD notice.
