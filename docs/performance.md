# Performance

How fast Crossfade converts FLAC, measured against the reference `flac` tool,
and how to repeat the measurement.

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
