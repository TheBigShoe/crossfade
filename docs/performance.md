# Performance

How fast Crossfade is against the reference tool for each job, what those
reference tools are and where to get them, and how to repeat the measurement.
Every table, the step-by-step history and what is left to gain are in
[performance-details.md](performance-details.md).

## Results

Wall time of the whole command, in milliseconds. Lower is better.

| Job | Input | `crossfade` | `crossfade --threads 1` | Reference tool | Reference time |
|---|---|---|---|---|---|
| FLAC encode, level 5 | 60 s, 16-bit stereo | 22.5 | 75.6 | `flac` (`flac -j8`) | 100.7 (24.4) |
| FLAC decode | the same, as FLAC | 21.4 | 45.8 | `flac -d` | 70.3 |
| MP3 decode | 5 min, 192 kbit/s | 202 | 207 | `mpg123` | 276 |
| AAC decode | 5 min, 256 kbit/s `.aac` | 172 | 169 | `afconvert` | 189 |
| AAC decode | 5 min, 192 kbit/s `.m4a` | 158 | 159 | `afconvert` | 181 |
| ALAC decode | 5 min, 16-bit `.m4a` | 125 | 405 | `afconvert` | 399 |
| Resample 44.1 to 48 kHz | 60 s, 16-bit stereo | 54 | 189 | `afconvert` | 733 |

- Crossfade is faster than the reference in every row with its default
  settings. On one thread it is faster everywhere except ALAC, where it is
  level with `afconvert` (405 against 399).
- FLAC files come out the same size as the reference's or smaller.
- The thread count never changes a byte of output.
- Vorbis has no reference tool in this list. In process it decodes 1.7 to 3.6
  times faster than the `lewton` crate on Xiph's test files.
- Opus has no reference tool installed to race. On one thread it decodes the
  RFC 8251 vectors at 400 to 1,700 times real time in process, and
  `crossfade convert` turns 25 to 30 s of Opus into FLAC in 0.03 to 0.09 s.

## The reference test

"Reference" means the established tool a person would otherwise use for the
same job. One test is one job done by both tools on the same input:

1. **Whole process against whole process.** The time from starting the command
   to its exit, reading a file and writing a file. In-process timings are never
   used for a claim, because they hide start-up and file I/O.
2. **Same input, same output.** Both tools read the same file and write the
   same format (decoders write a 16-bit WAV).
3. **Best of 9 or 11 runs, interleaved.** The commands take turns, so background
   load falls on all of them alike, and the fastest run of each is kept.
4. **Default settings**, plus `--threads 1` for a single-thread figure.

The commands:

| Job | Crossfade | Reference |
|---|---|---|
| FLAC encode | `crossfade convert in.wav -o out.flac --compression-level 5` | `flac -5 -s -f -o out.flac in.wav` |
| FLAC decode | `crossfade convert in.flac -o out.wav` | `flac -d -s -f -o out.wav in.flac` |
| MP3 decode | `crossfade convert in.mp3 -o out.wav` | `mpg123 -q -w out.wav in.mp3` |
| AAC and ALAC decode | `crossfade convert in.m4a -o out.wav` | `afconvert -f WAVE -d LEI16 in.m4a out.wav` |
| Resample | `crossfade convert in.wav -o out.wav --sample-rate 48000` | `afconvert -f WAVE -d LEI16@48000 -r 127 --src-complexity bats in.wav out.wav` |

Measured on an Apple M1 Max (8 performance and 2 efficiency cores), macOS
27.2.1, Rust 1.98.1, the default release profile.

## Getting the reference tools

| Tool | Version measured | Used for | How to get it |
|---|---|---|---|
| `flac` | 1.5.0 | FLAC encode and decode | `brew install flac`, or [xiph.org/flac](https://xiph.org/flac/download.html). 1.5 or later for `-j8`. |
| `mpg123` | 1.33.7 | MP3 decode | `brew install mpg123`, or [mpg123.de](https://www.mpg123.de/download.shtml) |
| `afconvert` | macOS 27.2.1 | AAC, ALAC and MP3 decode; resampling; making the AAC and ALAC test files | Ships with macOS: nothing to install (`man afconvert`). There is no Linux or Windows version. |
| `lame` | 4.0 | Making the MP3 test file only | `brew install lame`, or [lame.sourceforge.io](https://lame.sourceforge.io/) |
| `lewton` | 0.10.2 | Vorbis, in process only | A Rust crate that cargo fetches by itself as a test dependency ([github.com/RustAudio/lewton](https://github.com/RustAudio/lewton)) |

On macOS with Homebrew, one line installs everything that is not built in:

```sh
brew install flac mpg123 lame
```

These tools are for speed. Correctness is checked against the official
conformance files instead (`just corpus` fetches them; the README's Tests
section says which).

## Repeating it

```sh
scripts/compare-with-flac.py      # FLAC encode and decode against flac
scripts/compare-decoders.py       # MP3, AAC and ALAC decode against mpg123 and afconvert
```

Each script builds the release binary, makes its own test signal, encodes the
test files with the reference tools, times both sides and prints the table.
They need Python 3 and the tools above; a tool that is missing leaves its
column out. To measure your own audio, pass a 16-bit WAV:

```sh
scripts/compare-with-flac.py --reps 11 song.wav
scripts/compare-decoders.py song.wav
```

The AAC rows need a build with the `aac` feature, which the script turns on
(see the README: AAC decoding is off in a default build).

## Reading the results

- **One machine, one signal per row.** Expect a few percent of run-to-run
  noise. The FLAC and resampling rows were taken on a quiet machine, the
  decoder rows with other work running.
- **Threads.** FLAC, ALAC and the resampler use several threads by default;
  MP3 and AAC decode on one, which is why their two columns match.
- **What sets the pace.** FLAC is held by the MD5 of the audio, which the
  format requires and which cannot be split across threads. For MP3 and AAC,
  the dithered conversion to 16-bit is about a fifth of the time.
- **Not measured.** Linux, Windows and x86 machines. Vorbis against libvorbis
  and Opus against libopus (no tool installed).

Everything is safe Rust: every crate forbids `unsafe`, so there are no
hand-written SIMD intrinsics.
