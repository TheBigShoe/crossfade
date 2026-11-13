# Performance: full tables and history

The long form of [performance.md](performance.md): every table, how each
result was reached step by step, and what is left to gain. Start with the
short report; come here for the numbers behind a line of it. The reference
tools and how to get them are described there.

## Summary

On an Apple M1 Max (8 performance and 2 efficiency cores), `crossfade convert`
with its default settings:

- **encodes** FLAC 2.2 to 8.6 times faster than `flac` with its default single
  thread, and 1.1 to 1.4 times faster than `flac -j8`;
- **decodes** FLAC 1.9 to 3.4 times faster than `flac -d`;
- writes files the same size as the reference or smaller;
- on one thread (`--threads 1`) is still faster than `flac` in every case
  below, by 1.05 to 1.5 times when encoding and 1.25 to 1.7 times when decoding.

The thread count never changes a byte of output.

## What was measured

Whole process against whole process: the wall time of the command a person
would type, from start to exit, reading a file and writing a file. Each figure
is the best of 11 runs, with the commands interleaved so that background load
falls on all of them alike.

```sh
crossfade convert in.wav -o out.flac --compression-level N --force
flac -N -s -f -o out.flac in.wav

crossfade convert in.flac -o out.wav --force
flac -d -s -f -o out.wav in.flac
```

- Machine: Apple M1 Max, macOS 27.2.1. Rust 1.98.1, default release profile.
- Reference: `flac` 1.5.0 (Homebrew).
- Crossfade at commit `f635599`.
- Both tools compute the MD5 of the audio when encoding and check it when
  decoding.

Signals:

| Name | Content |
|---|---|
| `bench16` | 60 s, 16-bit, 44.1 kHz stereo: a mix of sines and seeded noise (the `flac_signals` example) |
| `bench24` | 10 s, 24-bit, 96 kHz stereo: the same kind of signal |
| `speech44_16` | 18 s of synthesised speech, 16-bit, 44.1 kHz stereo |
| `speech192_24` | the same speech at 24-bit, 192 kHz: band-limited material in a high-rate container |

## Encode (WAV to FLAC)

Wall time in milliseconds. "Size" is our audio data against the reference's
at the same level; negative is smaller.

| Signal | Level | crossfade | crossfade `--threads 1` | flac | flac `-j8` | Size |
|---|---|---|---|---|---|---|
| bench16 | 0 | 22.5 | 46.0 | 62.0 | 26.2 | -0.00% |
| bench16 | 5 | 22.5 | 75.6 | 100.7 | 24.4 | -0.00% |
| bench16 | 8 | 23.9 | 139.3 | 205.0 | 33.3 | -0.01% |
| bench24 | 0 | 13.9 | 25.1 | 32.6 | 15.7 | -0.00% |
| bench24 | 5 | 13.8 | 41.6 | 46.1 | 15.7 | -0.00% |
| bench24 | 8 | 14.3 | 69.5 | 93.8 | 17.9 | -0.00% |
| speech192_24 | 0 | 39.9 | 73.0 | 96.7 | 43.4 | -0.29% |
| speech192_24 | 5 | 37.9 | 122.3 | 128.2 | 42.5 | -7.44% |
| speech192_24 | 8 | 38.2 | 241.3 | 252.8 | 43.5 | -2.76% |
| speech44_16 | 0 | 9.6 | 18.0 | 21.1 | 10.6 | -0.14% |
| speech44_16 | 5 | 9.4 | 22.4 | 28.3 | 10.5 | -0.38% |
| speech44_16 | 8 | 10.6 | 47.5 | 52.5 | 11.6 | -0.49% |

## Decode (FLAC to WAV)

The input is the reference encoder's file at the level shown. The WAV audio
is identical to `flac -d`'s in every case.

| Signal | Level | crossfade | crossfade `--threads 1` | flac `-d` |
|---|---|---|---|---|
| bench16 | 0 | 21.8 | 37.5 | 57.5 |
| bench16 | 5 | 21.4 | 45.8 | 70.3 |
| bench16 | 8 | 21.1 | 50.7 | 72.0 |
| bench24 | 0 | 13.6 | 20.8 | 31.8 |
| bench24 | 5 | 14.0 | 23.6 | 31.2 |
| bench24 | 8 | 13.3 | 25.8 | 32.3 |
| speech192_24 | 0 | 38.2 | 63.2 | 105.5 |
| speech192_24 | 5 | 37.3 | 55.0 | 77.7 |
| speech192_24 | 8 | 37.4 | 58.3 | 80.7 |
| speech44_16 | 0 | 9.4 | 14.4 | 20.2 |
| speech44_16 | 5 | 9.2 | 12.5 | 17.5 |
| speech44_16 | 8 | 9.2 | 13.4 | 17.6 |

## Reading the numbers

- **The compression level barely changes the wall time.** With frames coded
  in parallel, the MD5 of the audio sets the pace: it is one serial chain,
  about 620 MB/s of PCM on this machine, and FLAC requires it. Level 8 costs
  more processor time than level 0, not more waiting.
- **Threads cost processor time.** Total CPU time for the same file is 10 to
  90% higher than on one thread, most at level 0, where frames are cheap and
  coordination is a larger share.
- **Default thread counts:** the encoder uses the machine's parallelism less
  three, between 2 and 8 (7 here); the decoder stops at 3, since more threads
  would only wait for the MD5. `--threads 1` starts no thread at all, and
  short files start none either.
- **Where we are not faster.** Measured on one thread, in process: a
  synthetic 192 kHz sine sweep encodes in 1.2 to 1.4 times the reference's
  time (for a file 17 to 22% smaller: an extra search on highly predictable
  audio is a deliberate trade), and full-scale 32-bit audio at level 8 runs
  at about 0.86 times the reference's speed. With the default threads both
  are 3 to 5 times faster than `flac`, whole process against whole process.
- These are one machine's figures on four signals. Differences of a few
  percent are within run-to-run noise.

## How it got here

In order of effect:

1. **Threads.** Frames are independent, so they are encoded and decoded on a
   small ordered worker pool (`crossfade-flac`'s `pool`, `std` only), with
   the MD5 on a thread of its own.
2. **The code around the codec.** File reads and writes are buffered and run
   on helper threads, and finished files are not force-flushed to disk before
   the rename into place (as `cp` and `flac` do not either). A 10 MB container
   copy went from 18 ms to 7 ms.
3. **The encoder.** Exact integer autocorrelation, residual kernels
   specialised by predictor order, candidate predictors scored by an exact
   covariance form so that only the most promising are coded, and fixed
   predictors skipped when a lower bound proves they cannot win.
4. **The decoder.** Prediction history kept in registers, Rice decoding on a
   local bit window, CRC-16 sixteen bytes a step.

Everything is safe Rust: every crate forbids `unsafe`, so there are no
hand-written SIMD intrinsics. The hot loops are written so the compiler
vectorises them under any release profile.

## Sample-rate conversion

Measured on the same Apple M1 Max, whole process against whole process, the
minimum of 9 interleaved runs (milliseconds). `afconvert` is macOS's, at its
best setting (`-r 127 --src-complexity bats`), writing the same format.
Crossfade uses its default quality (`good`, the resampler's `Balanced`) and
dithers integer output (TPDF).

| Conversion | `crossfade` | `crossfade --threads 1` | `afconvert` |
|---|---|---|---|
| bench16 (60 s, 16-bit stereo) 44.1 -> 48 kHz, WAV | 54 | 189 | 733 |
| bench16 44.1 -> 48 kHz, FLAC (level 5) | 73 | 266 | 1069 |
| bench24 (10 s, 24-bit stereo) 96 -> 44.1 kHz, WAV | 25 | 62 | 262 |

The `afconvert` column is from the previous measurement on the same machine
(it does not change with Crossfade). Wall time against CPU time (user plus
system) for the same runs, with a 60 s 24-bit 5.1 file (`six.wav`, bench16
mixed up to 5.1) as well:

| Conversion | 1 thread: wall | CPU | auto: wall | CPU |
|---|---|---|---|---|
| bench16 44.1 -> 48 kHz WAV | 189 ms | 191 ms | 54 ms | 214 ms |
| bench16 44.1 -> 48 kHz FLAC | 266 ms | 267 ms | 73 ms | 304 ms |
| bench24 96 -> 44.1 kHz WAV | 62 ms | 61 ms | 25 ms | 70 ms |
| six 44.1 -> 48 kHz WAV | 517 ms | 532 ms | 139 ms | 571 ms |
| six 44.1 -> 48 kHz FLAC | 719 ms | 727 ms | 210 ms | 800 ms |

The resampler splits the stream in time, not by channel, so stereo scales
past two threads: segments of at least 32,768 input frames start on a multiple
of the ratio's input period, each with a pre-roll and post-roll as long as
the filter's reach, converted by its own copy of the resampler; up to 8
segments run at once (`std::thread::scope`, one batch at a time). The bytes are
the same at every thread count. The extra CPU on auto is the overlap and the
threads' start-up, about 10 per cent. The budget is `--threads`: the
resampler uses all of it for its batches, which run while the pipeline thread
waits, so it shares the machine with a FLAC encoder's threads (which mostly
wait in a rate-changing job) rather than splitting the count with them.

By quality, bench16 44.1 -> 48 kHz to WAV: `fast` 108 ms, `good` 187 ms,
`best` 302 ms.

What processing costs on its own (bench16, no rate change): a plain WAV to WAV
conversion takes 6.1 ms; `--gain -3dB`, which runs the `f64` chain and TPDF
dither, 16.8 ms (16.5 ms without dither); to FLAC, 28.4 ms plain and 31.3 ms
with the gain.

Where the time went before threading: a `sample` profile of a 5-minute
conversion to FLAC at 48 kHz put about nine tenths of the main thread in the
resampler's dot product, a few per cent in the quantiser and the rest in
reading, MD5 and handing frames on; the FLAC encoder's threads mostly waited.

Quality, from the resampler's own measurements (`crates/crossfade-resample/README.md`):
passband flat within 0.00003 / 0.000004 / 0.00000005 dB, stopband at least
106 / 126 / 165 dB (fast / good / best). Through the engine (two real jobs, a
1 kHz sine at -1 dBFS, 44.1 -> 48 -> 44.1 kHz,
`round_trip_44k1_48k_44k1_snr_exit_criterion`): 142.9 / 159.4 / 239.5 dB with
64-bit float files in between, 142.4 / 153.2 / 154.8 dB with 32-bit float, and
139.0 / 141.0 / 141.0 dB with dithered 24-bit (one 24-bit quantisation alone
allows 140.5 dB).

## Analysis and the waveform in the app

Same machine, release builds.

- **Analysing a file** (decode, peak pyramid, loudness with range and true
  peak) took 7.2 s for one hour of 48 kHz 16-bit stereo, about 500 times real
  time, on one thread. One run.
- **The waveform's data for one frame of drawing** (both lanes of a stereo
  file, 1,700 columns) takes 20 to 28 microseconds on a one-hour file, whether
  the view shows the whole hour or one sample per pixel: the cost follows the
  width in pixels, never the length of the file. One hour of stereo holds
  21.6 MB of peaks.

Painting itself (GPUI drawing at most one shape per column) was judged from
offscreen renders, not timed, and has not been tried on a trackpad with an
hour-long file.

## ALAC and Vorbis decoding

One measurement each, on the same machine while other work was running, so
treat them as rough.

- **ALAC** (5 minutes of 16-bit 44.1 kHz stereo in `.m4a`, made by `afconvert`,
  whole process, best of 9, interleaved): `crossfade convert in.m4a -o out.wav`
  138 ms, with `--threads 1` 470 ms, `afconvert -f WAVE -d LEI16` 453 ms. Packets
  are independent, so the adapter decodes batches on up to four threads; the
  bytes are the same at every count. On one thread the adaptive predictor is
  about half the time.
- **Vorbis** has no reference tool installed here to race, so the comparison is
  in process against `lewton` 0.10.2, one thread, best of 5, on Xiph's test
  files: 1.7 to 3.6 times faster on every file (typically 2.5), 95 to 180
  million samples per second against 55. Residue decoding is about 43% of the
  time, the IMDCT 26%, floor curves and coupling 22%.

## MP3 decoding

Our own Layer III decoder (`crossfade-mp3`) against `mpg123` 1.33.7 and
Apple's `afconvert`, on the same machine. Three 300 s, 44.1 kHz files made
with LAME 4.0: CBR 192 kbit/s stereo, `-V2` stereo (VBR, 172 kbit/s on
average) and 64 kbit/s mono (`-m m -b 64`).

### Whole process

Wall time from start to exit, best of 9 with the commands interleaved, MP3
in and 16-bit WAV out (`crossfade` dithers, as it does by default for any
float to integer conversion), with the instructions and cycles that
`/usr/bin/time -l` counts for that run:

```sh
crossfade convert in.mp3 -o out.wav --force
mpg123 -q -w out.wav in.mp3
afconvert -f WAVE -d LEI16 in.mp3 out.wav
mpg123 -q -t in.mp3            # decode only, nothing written
```

| File | crossfade | `mpg123 -w` | `afconvert` | `mpg123 -t` | crossfade, `--sample-format f32 --dither none` |
|---|---|---|---|---|---|
| CBR 192 | **230.8 ms** (3112 Mins, 713 Mcyc) | 265.2 ms (2294 Mins, 794 Mcyc) | 422.7 ms | 219.6 ms (663 Mcyc) | 178.2 ms (578 Mcyc) |
| V2 | **220.8 ms** (3014 Mins, 677 Mcyc) | 253.0 ms (2149 Mins, 758 Mcyc) | 402.5 ms | 211.1 ms (624 Mcyc) | 168.1 ms (548 Mcyc) |
| Mono 64 | **108.2 ms** (1462 Mins, 318 Mcyc) | 139.3 ms (1206 Mins, 402 Mcyc) | 207.6 ms | 117.3 ms (338 Mcyc) | 84.0 ms (249 Mcyc) |

`crossfade` is 13 to 22% faster than `mpg123 -w` and about twice as fast as
`afconvert`. The last column writes the decoder's floats as they are: the
difference to the first (about 50 ms) is the dithered conversion to 16-bit,
which belongs to the engine, not the decoder. Before this work the CBR 192
file took 297 ms against `mpg123 -w`'s 266 ms.

### In process

The decoder alone, one thread, frames already in memory: best of 9 decodes
of the whole file (`crossfade-mp3`'s example `bench`), against `mpg123 -t`'s
whole process above (which decodes and writes nothing):

| File | crossfade-mp3 | Times real time | `mpg123 -t` (whole process) | Times real time |
|---|---|---|---|---|
| CBR 192 | 159.3 ms (2086 Mins, 512 Mcyc) | 1884 | 219.6 ms | 1366 |
| V2 | 149.9 ms (1987 Mins, 483 Mcyc) | 2002 | 211.1 ms | 1421 |
| Mono 64 | 66.0 ms (924 Mins, 213 Mcyc) | 4547 | 117.3 ms | 2558 |

Before: 1280 (CBR 192) and 1377 (V2) times real time.

### How it got here

Each step is its own commit with its numbers; in process, one thread, cycles
per decode of the CBR 192 / V2 / mono 64 files:

| Step | CBR 192 | V2 | Mono 64 | Output |
|---|---|---|---|---|
| Before | 741 Mcyc (230 ms) | 690 Mcyc (219 ms) | 367 Mcyc (114 ms) | |
| `F4::splat` written out: `[x; 4]` had become calls to `memset_pattern16` inside the kernels (about 10% of the time) | 659 | 608 | 333 | same bits |
| Synthesis windowing in matrix form: contiguous 4-wide sums over 16 slots, the DCT writing each slot's rows directly | 592 | 549 | 242 | last-bit changes, below |
| Alias butterflies, IMDCT and inversion skipped for subbands that are all zero (and whose overlap is) | 584 | 531 | 229 | same bits |
| Huffman: sign bits without a branch, a 10-bit first lookup per code tree | 509 | 473 | 211 | same bits |

The rows were measured one step at a time on a machine with other work
running, so consecutive figures can differ by a few percent from run to run.
The synthesis rewrite sums in another order: the largest change in any sample
is 1.2e-7 of full scale, and all 45 ISO/IEC 11172-4 conformance pairs keep
full accuracy, their RMS difference from the references 0.7% higher on
average (3.2% at most), two orders of magnitude inside the limit. The output
is the same bits with `CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1` (which is also
about 2% faster).

```sh
cargo run --release -p crossfade-mp3 --example bench -- in.mp3 9   # best of 9, digest
```

### What is left

A time profile (Instruments, CBR 192 / mono 64) of the decoder as it stands:
Huffman decoding 46% / 39%, the synthesis filterbank (DCT and windowing) 27% /
34%, the IMDCT 16% / 15%, requantisation, alias reduction, inversion and the
rest about 10%. The Huffman loop is now limited by its chain of dependent
loads (bits, lookup, length, next position); a bit cache refilled only when
short (instead of every pair) and a reader that refills ahead were both tried
and were no faster. Left to try: decoding a pair and its sign bits from one
lookup, an IMDCT without strided gathers, and fused multiply-add, which
determinism rules out (the same bits on every platform). In the whole process
the dithered conversion to 16-bit costs about as much as the IMDCT. The
output was not checked on x86_64: the Rosetta run of the bench hung at launch
on this machine and was killed.

## AAC decoding

AAC-LC to 16-bit WAV against macOS's own decoder, whole process against whole
process on the same Apple M1 Max:

```sh
crossfade convert in.aac -o out.wav --force          # default threads, and --threads 1
afconvert -f WAVE -d LEI16 in.aac out.wav
```

Wall time is the best of 9 runs with the three commands interleaved;
instructions retired and cycles are the smallest of the same runs from
`/usr/bin/time -l` (they move far less than wall time on this machine, and
count every thread). Crossfade at the tip of `m5/aac-speed`, the default
release profile, `--features aac`; "main before" is the same command built
from `main` before this work. The inputs are one 4 min 22 s signal (chords,
noise and clicks, 44.1 kHz stereo) encoded by `afconvert -f adts -d aac` at
256 and 96 kbit/s and by `afconvert -f m4af -d aac -b 192000`. Crossfade
dithers its 16-bit output (TPDF), so the quantiser is part of its time; its
AAC decode is bit for bit what it was before.

| Input | Command | Wall (ms) | Instructions | Cycles |
|---|---|---|---|---|
| 256 kbit/s ADTS | crossfade | 197 | 2.39e9 | 0.58e9 |
| | crossfade `--threads 1` | 182 | 2.38e9 | 0.57e9 |
| | afconvert | 223 | 1.59e9 | 0.61e9 |
| | main before | 425 | 4.60e9 | 1.31e9 |
| 96 kbit/s ADTS | crossfade | 147 | 1.91e9 | 0.45e9 |
| | crossfade `--threads 1` | 148 | 1.91e9 | 0.44e9 |
| | afconvert | 147 | 1.20e9 | 0.40e9 |
| | main before | 289 | 3.80e9 | 0.90e9 |
| 192 kbit/s M4A | crossfade | 192 | 2.24e9 | 0.55e9 |
| | crossfade `--threads 1` | 186 | 2.23e9 | 0.54e9 |
| | afconvert | 189 | 1.46e9 | 0.51e9 |
| | main before | 360 | 4.24e9 | 1.12e9 |

The thread count does not matter: AAC decodes on the pipeline thread, and a
second thread writes the file either way.

What made the difference, measured with the in-process benchmark
(`cargo run --release -p crossfade-aac --features decoder --example bench -- in.aac`,
which prints a hash of the output so a change that alters a bit shows),
cycles for the 256 / 96 kbit/s files: 1.10 / 0.69 e9 on `main`, then

- a bit reader holding a 64-bit word (on its own no faster, but it lets one
  peek cover a whole codeword and its sign bits);
- Huffman entries that carry a codeword's coefficients, which of them take a
  sign bit and the total length, so a codeword is one lookup; the sign applied
  by flipping the sign bit rather than by a branch on a random sign (the
  largest single gain), and the band loop working on a register copy of the
  reader: 0.69 / 0.56 (measured with the old, serial hash);
- the FFT's butterfly stages written so they vectorise, the pre-twiddle in
  natural order with the bit reversal folded into the FFT's first pass:
  0.49 / 0.34;
- windowing and overlap-add in one pass into a planar buffer, then one
  interleaving pass; intensity-stereo gains from a table: 0.41 / 0.27;
- the engine side: the adapter decodes straight into the frame it hands out
  (and reuses frames, which it meant to and did not), and the TPDF quantiser's
  arithmetic vectorises (converting through `i64`, clamping as an integer,
  counting clips in integer lanes; the generator's draws run a block ahead,
  the same draws in the same order).

Every step kept the output bit for bit (the hashes, the frozen digests and the
ISO conformance tier), except one deliberate change: noise substitution is now
seeded from each block's bytes, so a seek gives the same noise as a straight
run.

What is left, from a time profile of the whole process on a 25-minute
96 kbit/s file (share of samples, both threads): spectral parsing and
dequantisation (`Ics::read`) 24%, the IMDCT 22%, the quantiser 18%, the
kernel writing the file on the I/O thread 13%, window and overlap-add 3%,
`f32` to `f64` conversion before the quantiser 3%, the rest small. At
256 kbit/s parsing grows to 43%. The quantiser is now mostly SplitMix64's
two 64-bit multiplies per sample, which have no vector form without `unsafe`;
the parse is bound by the chain from one codeword's length to the next lookup.
What could still be had: merging two FFT stages per pass (tried; written
safely it did not vectorise and lost), folding the post-twiddle into the
window-and-add pass, and quantising straight from `f32` without the `f64`
working buffer when a chain only quantises.

## Repeating it

```sh
scripts/compare-with-flac.py                 # bench16 and bench24, levels 0, 5, 8
scripts/compare-with-flac.py --all           # more synthetic signals
scripts/compare-with-flac.py --reps 11 a.wav # your own files as well
```

The script builds the release binary, generates the synthetic signals, runs
both tools and prints the tables above. It needs Python 3 and `flac` on the
path (1.5 or later for the `-j8` column). The speech files were made on macOS
with `say -o speech.aiff ...` and `afconvert -f WAVE -d LEI24@192000`.

In-process figures, without process start or file I/O:

```sh
cargo bench -p crossfade-flac
cargo run --release -p crossfade-flac --example flac_encode -- --time 9 --level 5 in.wav out.flac
cargo run --release -p crossfade-format --example profile_flac_read -- in.flac
```
