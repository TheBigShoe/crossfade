# crossfade-loudness

Programme loudness and peak measurement in pure Rust, to ITU-R BS.1770-4 and EBU R 128:
integrated, momentary and short-term loudness (EBU Tech 3341), loudness range (Tech 3342),
sample peak and true peak, any sample rate from 8,000 to 1,048,575 Hz, and a helper for the
gain that normalises a programme under a true-peak ceiling. No `unsafe`; the only dependency
is `libm`.

```rust
use crossfade_loudness::{ChannelRole, Meter, Options, normalisation_gain};

let roles = ChannelRole::standard_layout(2).unwrap();
let mut meter = Meter::new(48_000, roles, Options::default())?;
meter.push(&interleaved_samples);      // any length, even half a frame
println!("{:?} LUFS", meter.momentary()); // readable at any time
let m = meter.finish();                // integrated, M/S and their maxima, LRA, peaks
let gain = normalisation_gain(m.integrated, -16.0, m.true_peak.unwrap_or(1.0), -1.0);
```

## Same bits on every platform

A loudness figure feeds a normalising gain, which changes output bytes. `std`'s `sin`, `cos`,
`tan`, `exp`, `ln`, `log10` and `powf` are not guaranteed to agree between platforms, so the
crate uses none of them and no `mul_add`: `+ - * /`, `sqrt`, comparisons, conversions and
`libm` calls only. `tests/pinned.rs` holds the exact `f64` bits of one measurement (also
checked with one codegen unit and in release). Results are also bit-identical however the
input is split across `push` calls (a property test).

## What is implemented

- **K-weighting**, derived for the actual rate. The standard prints coefficients for 48 kHz
  only. They are the bilinear transform of two analogue filters, a high shelf (1681.97 Hz,
  +4.0 dB, Q 0.7072) and a high-pass (38.135 Hz, Q 0.5003); those parameters, recovered from
  the published tables, are pre-warped and re-transformed for any rate. At 48 kHz the result
  equals BS.1770-4 Tables 1 and 2 to better than 1e-9 (tested).
- **Integrated loudness**: weights L/R/C 1.0, surrounds 1.41, LFE not measured; 400 ms blocks
  every 100 ms; absolute gate -70 LUFS, relative gate 10 LU under the gated mean.
- **Momentary** (400 ms) and **short-term** (3 s): current value and maximum, updated every
  20 ms (finer than the 10 Hz minimum, which Tech 3341 cases 13 and 14 need: a 400 ms tone
  starting 20 ms off a 100 ms grid would read 0.2 LU low).
- **Loudness range** per Tech 3342: 3 s blocks every 100 ms, absolute gate -70 LUFS, relative
  gate -20 LU, 95th minus 10th percentile, ranked as the Tech 3342 MATLAB listing does.
- **Sample peak** and **true peak**, per channel and overall (see below).
- Channel roles by the caller: `Left`, `Right`, `Centre`, `LeftSurround`, `RightSurround`,
  `Lfe` (no loudness, peaks measured), `Unweighted` (1.0, for mono or any other full-range
  channel), `Excluded` (nothing). `ChannelRole::standard_layout(n)` for 1 to 6 channels.
- **Mono** counts once. Tech 3341 and 3342 contain no "dual mono" +3 LU convention, so none is
  offered; push the channel twice to measure it as two loudspeakers.
- `reset()`, `Options { true_peak, loudness_range }` to switch off the costly parts.

### True peak

Our own interpolator, no dependency: a Kaiser-windowed sinc (beta 6), 12 taps per phase,
4x below 88.2 kHz, 2x below 176.4 kHz, none above, each phase normalised to unit DC gain,
built at construction from `libm`. Phase 0 is the sample itself, so the true peak never reads
below the sample peak. The tail is included as if the signal ended at the last sample.
Measured against an ideal interpolator over frequency and phase (the test): never more than
0.03 dB under the best any L-times oversampler can do (the crest can fall between two
oversampled points: 0.17 dB at fs/4 with 4x) and never more than 0.006 dB over the true level.

### Silence, short input, bad input

Loudness values are `Option<f64>`; `None` means no loudness: under 400 ms pushed (3 s for
short-term and range), digital silence, everything under -70 LUFS. Peaks are linear, `0.0` for
silence (`to_db` gives negative infinity). Never NaN. NaN and infinities count as 0.0 and
finite samples beyond +-1e12 are clamped; `sanitised_samples()` counts them.

### Memory

Samples are not stored. One 8-byte power per 100 ms for the gate and one for the range: 16
bytes per 100 ms, about 14 MB for 24 hours (a `Vec` may hold twice that); silence stores
nothing. Tested for an hour, and for a full day with `cargo test --release -- --ignored`.

## EBU cases, synthesised and measured

All at 48 kHz, stereo 1 kHz sines unless stated. Loudness tolerance +-0.1 LU. Measured values
are `-23.0`-relative LUFS; the 0.007 LU offset at 1 kHz is the filter, not noise.

| Case | Signal | Expected | Measured |
|---|---|---|---|
| 3341 #1 | -23 dBFS, 20 s | M, S, I = -23.0 | -22.993 each |
| #2 | -33 dBFS, 20 s | M, S, I = -33.0 | -32.993 each |
| #3 | 10 s -36, 60 s -23, 10 s -36 | I = -23.0 | -23.014 |
| #4 | -72/-36/-23/-36/-72, 10 s, 10 s, 60 s, 10 s, 10 s | I = -23.0 | -23.014 |
| #5 | 20 s -26, 20.1 s -20, 20 s -26 | I = -23.0 | -22.979 |
| #6 | 5.0: L R -28, C -24, Ls Rs -30, 20 s | I = -23.0 | -23.016 |
| #9 | (1.34 s -20, 1.66 s -30) x5 | S = -23.0 after 3 s | -22.986, constant |
| #10 | i*0.15 s silence, 3 s -23, 1 s silence, i = 0..19 | max S = -23.0 | -23.008 to -22.993 |
| #11 | live sequence, -38+i dBFS | max S = -38 ... -19 | -37.993 ... -19.008 |
| #12 | (0.18 s -20, 0.22 s -30) x25 | M = -23.0 after 1 s | -22.960, constant |
| #13 | i*20 ms silence, 400 ms -23, 1 s silence | max M = -23.0 | -22.994 to -22.960 |
| #14 | live sequence, -38+i dBFS | max M = -38 ... -19 | -37.994 ... -18.994 |
| 3341 #15 | fs/4, 0.5, phase 0 | -6.0 dBTP, +0.2/-0.4 | -6.021 |
| #16 | fs/4, 0.5, 45 deg | -6.0 | -6.021 |
| #17 | fs/6, 0.5, 60 deg | -6.0 | -6.018 |
| #18 | fs/8, 0.5, 67.5 deg | -6.0 | -6.021 |
| #19 | fs/4, 1.41, 45 deg | +3.0 | +2.983 |
| #20-#23 | fs/6 with one fs/4 period, made at 4 fs, low-passed, decimated at offsets 0-3 | 0.0 | -0.130, -0.083, -0.195, -0.083 |
| 3342 #1 | -20 dBFS 20 s then -30 dBFS 20 s | LRA 10 +-1 | 10.0 |
| #2 | -20 then -15 | 5 +-1 | 5.0 |
| #3 | -40 then -20 | 20 +-1 | 20.0 |
| #4 | -50, -35, -20, -35, -50, 20 s each | 15 +-1 | 15.0 |

The EBU's signals are defined by construction and made here from nothing; the case 20-23
low-pass is a 257-tap windowed sinc in the test. Short tones are 1 kHz at integer cycles per
millisecond, so segment edges are exact.

**Not covered** (they are recorded programmes; the files are not downloaded or committed):
Tech 3341 cases 7 and 8 (authentic narrow- and wide-range programmes, I = -23.0) and
Tech 3342 cases 5 and 6 (the same programmes, LRA 5 and 15).

Other tests: the K-weighting curve at 48 kHz against the published filter, the same signal
at 44.1, 48, 88.2, 96, 176.4 and 192 kHz within 0.05 LU, unusual rates (8 kHz to
1,048,575 Hz), channel weights, gating edges, silence, a 0.3 s file, non-finite input,
chunking invariance, the gain helper, and the pinned bits. `fuzz/fuzz_targets/loudness.rs`
covers arbitrary rates, roles, chunkings and sample values.

## Speed

`cargo bench -p crossfade-loudness`, 60 s of stereo 48 kHz noise, one thread (Apple silicon):
all measurements on about 100 ms, roughly 600 times real time; integrated loudness only
about 41 ms, roughly 1,470 times. Of the all-on time, 60% is the true-peak interpolator.

## Gain helper

`normalisation_gain(integrated, target_lufs, true_peak, ceiling_dbtp)` returns the linear
factor to reach the target, reduced if the true peak would pass the ceiling; unity when there
is no loudness, a NaN target, or a NaN ceiling.
