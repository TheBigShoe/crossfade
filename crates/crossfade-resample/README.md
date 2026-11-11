# crossfade-resample

Sample-rate conversion in pure Rust: band-limited (windowed-sinc, linear-phase) polyphase
interpolation between any two rates from 1 Hz to 1,048,575 Hz, streaming, for 1 to 256
channels of `f64`. Its only dependency is `libm`; no `unsafe`.

```rust
use crossfade_resample::{Quality, Resampler};

let mut r = Resampler::new(44_100, 48_000, 2, Quality::Balanced)?;
let mut out = Vec::new();
for chunk in interleaved_chunks {
    r.process(chunk, &mut out)?; // any number of whole frames, empty included
    // ... send `out` on and clear it
}
r.flush(&mut out); // the tail; `r` is then ready for a new stream
```

`process_planar` and `flush_planar` take one slice and one vector per channel instead.

## Contract

- **Time-aligned.** The filter's delay is compensated: output frame `n` is the signal at
  `n / output_rate` seconds, output 0 is input 0, and silence is assumed before and after.
- **Length.** `N` input frames give exactly `output_len(N) = ceil(N * out / in)` frames: one
  for every output instant in `[0, N / in_rate)`. (Rounding up keeps the last partial input
  period; a round trip can therefore add one frame.)
- **Latency.** `latency()` is the lookahead in input frames: nothing comes out until
  `latency() + 1` frames have gone in, then output keeps pace. `flush` writes the remaining
  frames as if silence followed.
- **Chunking never changes the output**: the bits are identical however the input is split,
  interleaved or planar (a property test and the fuzz target check it).
- **The same bits on every platform.** Filter design uses only `+ - * /`, `sqrt` and `libm`
  (`std`'s `sin`, `exp` and `powf` are not guaranteed to agree across platforms); nothing
  uses fused multiply-add; every sum has its order written in the source (the dot product
  keeps 16 independent lanes combined in a fixed tree, so vectorisation cannot reorder it).
  A test pins the FNV-1a hash of five fixed conversions, one per code path; debug, release
  and `codegen-units = 1` builds agree.
- **Equal rates** pass samples through bit for bit, with zero latency.
- **DC** passes with gain 1 to within 1e-12 (every exact table row, or class of rows, is
  normalised; interpolated transposed tables divide by each output's summed weights).
- **Non-finite input** is not an error. A NaN or infinity spoils the outputs whose kernel
  reaches it (about `latency()` input frames either side) and the stream then recovers
  exactly; finite input within ±1e300 always gives finite output; nothing panics.
- **`reset()`** forgets the stream (history, accumulators, position) and keeps every
  allocation; `flush` ends with the same reset.

## Design

The ratio is reduced by its GCD to `up / down` (44.1 -> 48 kHz is 160 / 147).

- **Kernel**: a sinc under a Kaiser window with its pedestal removed, `(I0(β√(1-x²)) - 1) /
  (I0(β) - 1)`, so the kernel reaches zero continuously at its ends (interpolated tables stay
  accurate there, and the transposed form's per-output sums converge fast). The cut-off is
  midway through the transition band, which ends at the lower rate's Nyquist frequency, so
  images and aliases of everything start there. β follows Kaiser's formula for the design
  attenuation; the length is Kaiser's estimate at the lower rate, rounded up to a multiple of
  16 taps. Downsampling stretches the kernel in time by the ratio.
- **Direct form** (upsampling, and decimation up to 64:1): each output is the dot product of
  one table row with the input history around its instant; stereo pairs share the coefficient
  loads.
- **Transposed form** (decimation beyond 64:1, e.g. 192 kHz -> 1 kHz or 1,048,575 Hz ->
  7 Hz): each input is added into the outputs its kernel reaches, which are written once no
  later input can reach them. Memory follows the kernel's width in output samples instead of
  input samples (the direct form would hold 96 × 1,048,575 samples per channel for
  1,048,575 Hz -> 1 Hz).
- **Phase tables**: every phase is held exactly while phases × taps ≤ 2^18 coefficients (every
  common pair: 44.1 <-> 48 kHz has 160 phases). Past that (44.1 -> 44.101 kHz has 44,101
  phases, 1 Hz -> 1,048,575 Hz a million) the table holds 128 / 256 / 512 rows per sample at
  the lower rate (Fast / Balanced / Best) and interpolates between them with a 4-point
  Lagrange cubic, whose error (falling as the fourth power of the row spacing) stays under
  each stopband: the tests measure the stated stopband through interpolated tables.
- **Bounds**: construction evaluates the kernel at most 2^18 times: 2.6 ms for 44.1 -> 48 kHz
  Balanced, 17 ms for Best with interpolated tables, and under 45 ms for any pair on an Apple
  M1 Max (a scan of 576 pairs per preset).
  `memory_bytes()` reports what a resampler holds: at most 2.4 MiB of tables plus 190 KiB per
  channel for any rates (Best at 64:1 decimation); 44.1 -> 48 kHz Balanced holds 0.25 MiB
  plus 10 KiB per channel. Nothing grows after construction; `process` allocates only the
  caller's output growth.

## Specification

Measured by the tests (`tests/spec.rs`, `tests/awkward.rs`) and `examples/spec.rs`
(`cargo run --release -p crossfade-resample --example spec`). Frequencies are fractions of the
lower rate's Nyquist frequency; the stopband starts at that Nyquist frequency.

| | Fast | Balanced (default) | Best |
|---|---|---|---|
| Passband edge | 0.85 (18.7 kHz at 44.1) | 0.91 (20.07 kHz) | 0.935 (20.6 kHz) |
| Transition band | 0.85 - 1.0 | 0.91 - 1.0 | 0.935 - 1.0 |
| Passband ripple, stated | ±0.001 dB | ±0.0001 dB | ±0.000001 dB |
| Passband ripple, measured | ±0.00003 dB | ±0.000004 dB | ±0.00000005 dB |
| Stopband, stated | ≥ 100 dB | ≥ 120 dB | ≥ 160 dB |
| Stopband, measured worst (at its edge) | 106 dB | 126 dB | 165 dB |
| 1 kHz -1 dBFS round trip 44.1 -> 48 -> 44.1 kHz SNR, stated | ≥ 130 dB | ≥ 145 dB | ≥ 200 dB |
| Same, measured (vs the original, sample by sample) | 143 dB | 159 dB | 239 dB |
| Taps at the lower rate (design attenuation) | 96 (110 dB) | 192 (130 dB) | 352 (170 dB) |
| Latency at 44.1 -> 48 kHz | 48 frames (1.1 ms) | 96 frames (2.2 ms) | 176 frames (4.0 ms) |

The design attenuation is the largest whose Kaiser length still rounds up to the same tap
count, so upsampling costs nothing extra; decimating rounds up least, which is where the
stated stopband was measured. The default meets the milestone minimums with margin: flat
within ±0.01 dB to 20 kHz (±0.000004 dB measured), images and aliases ≥ 110 dB down
(126 dB), round-trip SNR ≥ 110 dB (159 dB, the named test
`round_trip_44k1_48k_snr_exit_criterion`); `Best` passes 140 dB with 239 dB.

**Overshoot.** Band-limiting rings: full-scale square waves (997 Hz to 5.5 kHz) come out at
up to 1.446 (+3.2 dB), on 48 -> 44.1 kHz. The hard bound for any input within ±1, the
largest sum of |coefficients| one output uses, is 2.42 / 2.78 / 3.04 (Fast / Balanced /
Best), reached only by an input built to hit it. Anything converting the result to integers
must clip (or leave that headroom).

**Against macOS `afconvert`** (`-r 127 --src-complexity bats`, 64-bit float in and out;
development only, no test calls it). On 44.1 -> 48 kHz tones `afconvert` is also
time-aligned with the same length rule (71,331 frames from 65,536). It keeps a wider
passband (flat to 21 kHz, -2.8 dB at 21.8 kHz, where Balanced is at -4.7 dB at 21 kHz) and a
flat spurious floor near -147 dB at every frequency. Balanced's images run from -167 dB at
1 kHz to -131 dB at the top of the transition band; Best's stay below -169 dB everywhere.
On 48 -> 44.1 kHz, tones at 22.5 and 23 kHz come out at -148 dB from `afconvert`, -133 dB
from Balanced. A 1 kHz round trip 44.1 -> 48 -> 44.1 kHz leaves noise and distortion
144 dB below the tone with `afconvert`, 163 dB with Balanced. So `afconvert` at its best sits
between Balanced and Best in rejection, with a wider passband than either.

## Speed

One thread, stereo, 60 s of audio in 4096-frame chunks, best of 7 runs on an Apple M1 Max
(`cargo run --release -p crossfade-resample --example throughput`); times real time:

| Pair | Fast | Balanced | Best |
|---|---|---|---|
| 44.1 -> 48 kHz | 87 ms, 690x | 156 ms, 385x | 268 ms, 224x |
| 48 -> 44.1 kHz | 90 ms, 666x | 152 ms, 396x | 267 ms, 225x |
| 96 -> 44.1 kHz | 154 ms, 389x | 286 ms, 209x | 511 ms, 117x |
| 44.1 -> 96 kHz | 174 ms, 344x | 310 ms, 194x | 538 ms, 112x |
| 192 -> 48 kHz | 293 ms, 205x | 557 ms, 108x | 987 ms, 61x |
| 44.1 -> 44.101 kHz (interpolated) | 199 ms, 301x | 340 ms, 176x | 573 ms, 105x |

The same with `CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1`:

| Pair | Fast | Balanced | Best |
|---|---|---|---|
| 44.1 -> 48 kHz | 87 ms, 691x | 157 ms, 382x | 269 ms, 223x |
| 48 -> 44.1 kHz | 90 ms, 666x | 152 ms, 395x | 267 ms, 225x |
| 96 -> 44.1 kHz | 154 ms, 389x | 288 ms, 208x | 516 ms, 116x |
| 44.1 -> 96 kHz | 173 ms, 346x | 308 ms, 195x | 536 ms, 112x |
| 192 -> 48 kHz | 292 ms, 205x | 559 ms, 107x | 988 ms, 61x |
| 44.1 -> 44.101 kHz (interpolated) | 199 ms, 301x | 339 ms, 177x | 574 ms, 105x |

The default, 44.1 -> 48 kHz stereo, runs at 385 times real time (the budget is 100, the aim
250): about 7 G multiply-adds a second through a 16-lane dot product that the compiler
vectorises into eight two-lane accumulators under either profile.
`cargo bench -p crossfade-resample` has the criterion baselines (10 s per case). Cost is
taps × outputs × channels for the direct form and taps × inputs × channels for the
transposed one: decimation by r costs about r times the work per output. Interpolated
tables add a 4-row blend per output frame, shared by the channels.

No threads: channels are independent, so a pool could convert channel pairs in parallel for
close to linear speed-up with the channel count (stereo would gain little); within one
channel, splitting the stream needs the kernel's overlap at every seam.

## Licence

MIT OR Apache-2.0.
