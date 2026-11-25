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

- In every row of this table Crossfade is faster than the reference with its default
  settings. On one thread it is faster everywhere except ALAC, where it is
  level with `afconvert` (405 against 399).
- FLAC files come out the same size as the reference's or smaller.
- The thread count never changes a byte of output.
- Vorbis has no reference tool in this list. In process it decodes 1.7 to 3.6
  times faster than the `lewton` crate on Xiph's test files.
- Vorbis has no reference tool in this list, and the two encoders (Apple
  Lossless, Opus) and Opus decoding were measured on another machine: see the
  next section, where Crossfade is slower than its reference in most rows.
- On one thread Opus decodes the RFC 8251 vectors at 400 to 1,700 times real
  time in process, and `crossfade convert` turns 25 to 30 s of Opus into FLAC in
  0.03 to 0.09 s.

## Results: the encoders and Opus decoding (Linux x86-64)

A different machine from the table above, so none of these figures may be
compared with it, and the ratios between the two tools are the only thing to
read across rows. Intel Xeon @ 2.10 GHz (4 cores, one thread each, a cloud VM
with AVX2 and AVX-512 available), 16 GB, Linux 6.18, Rust 1.98.1, the default
release profile. References: `opusenc` and `opusdec` from opus-tools 0.2 with
libopus 1.4, and `ffmpeg` 6.1.1 (libavcodec 60.31) for Apple Lossless. Wall time of
the whole command, in milliseconds, best of 11 interleaved runs, then the lowest
of several such series (four for the 16-bit music encodes, two for the other encodes
and for the music decode, one for the synthetic decode; the machine is a shared VM
whose speed moved by up to 10% between series, both tools' times together).
Lower is better.

| Job | Input | `crossfade` | `crossfade --threads 1` | Reference tool | Reference time |
|---|---|---|---|---|---|
| Apple Lossless encode | 256 s music, 16-bit 44.1 kHz stereo | 6,009 | 5,987 | `ffmpeg -c:a alac` | 1,634 |
| Apple Lossless encode | 60 s synthetic, 16-bit stereo | 1,431 | 1,431 | `ffmpeg -c:a alac` | 392 |
| Apple Lossless encode | 41 s music, 24-bit 96 kHz stereo | 4,049 | 4,041 | `ffmpeg -c:a alac` | 676 |
| Opus encode, 64 kbit/s | 256 s music, 16-bit 44.1 kHz stereo | 3,232 | 4,125 | `opusenc` | 2,924 |
| Opus encode, 128 kbit/s | 256 s music, 16-bit 44.1 kHz stereo | 3,978 | 4,938 | `opusenc` | 3,452 |
| Opus encode, 64 kbit/s | 60 s synthetic, 16-bit stereo | 738 | 947 | `opusenc` | 679 |
| Opus encode, 128 kbit/s | 60 s synthetic, 16-bit stereo | 899 | 1,092 | `opusenc` | 770 |
| Opus encode, 64 kbit/s | 41 s music, 24-bit 96 kHz stereo | 603 | 945 | `opusenc` | 541 |
| Opus encode, 128 kbit/s | 41 s music, 24-bit 96 kHz stereo | 744 | 1,071 | `opusenc` | 606 |
| Opus decode (128 kbit/s file) | 256 s music, to 16-bit WAV | 1,009 | 1,005 | `opusdec` | 2,258 |
| Opus decode (128 kbit/s file) | 300 s synthetic, to 16-bit WAV | 1,269 | 1,292 | `opusdec` | 2,620 |

- **Apple Lossless encode: Crossfade is 3.6 to 6 times slower than `ffmpeg`**, with
  its default settings and on one thread alike (the two columns match; its
  user time equals its wall time). It writes a file 0.5 to 1.5% smaller.
- **Opus encode: Crossfade is 9 to 23% slower than `opusenc`** with its default
  settings, and 39 to 77% slower on one thread. The default is faster than
  `--threads 1`: its user time exceeds its wall time, so more than one thread
  is busy. Bitrates come out within 1% of `opusenc`'s (see the details).
- **Opus decode: Crossfade is 2.1 to 2.2 times faster than `opusdec`**, on one
  thread or several.
- "Music" is 256 s (16-bit) and 41 s (24-bit, 96 kHz, so Opus converts it to
  48 kHz first) cut from the IETF FLAC test files, joined end to end; it is not
  commercial music. The synthetic signal is the same `bench16` the FLAC rows use.
- `afconvert`, which Apple Lossless is normally compared with, exists only on
  macOS. Its rows are to be measured there with the same script
  (`scripts/compare-encoders.py`) and added to a table of their own, never
  to this one.

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
| Apple Lossless encode | `crossfade convert in.wav -o out.m4a` | `ffmpeg -v error -y -i in.wav -c:a alac out.m4a` (macOS: `afconvert -f m4af -d alac`) |
| Opus encode | `crossfade convert in.wav -o out.opus --bitrate 128k` | `opusenc --quiet --bitrate 128 in.wav out.opus` |
| Opus decode | `crossfade convert in.opus -o out.wav` | `opusdec --quiet in.opus out.wav` |

The first table was measured on an Apple M1 Max (8 performance and 2 efficiency cores), macOS
27.2.1, Rust 1.98.1, the default release profile; the encoder and Opus decode
table on the machine described above it.

## Getting the reference tools

| Tool | Version measured | Used for | How to get it |
|---|---|---|---|
| `flac` | 1.5.0 | FLAC encode and decode | `brew install flac`, or [xiph.org/flac](https://xiph.org/flac/download.html). 1.5 or later for `-j8`. |
| `mpg123` | 1.33.7 | MP3 decode | `brew install mpg123`, or [mpg123.de](https://www.mpg123.de/download.shtml) |
| `afconvert` | macOS 27.2.1 | AAC, ALAC and MP3 decode; resampling; making the AAC and ALAC test files | Ships with macOS: nothing to install (`man afconvert`). There is no Linux or Windows version. |
| `lame` | 4.0 | Making the MP3 test file only | `brew install lame`, or [lame.sourceforge.io](https://lame.sourceforge.io/) |
| `opusenc`, `opusdec` | opus-tools 0.2, libopus 1.4 | Opus encode and decode | `brew install opus-tools`, `apt install opus-tools`, or [opus-codec.org](https://opus-codec.org/downloads/) |
| `ffmpeg` | 6.1.1 | Apple Lossless encode, on Linux | `apt install ffmpeg`, `brew install ffmpeg`, or [ffmpeg.org](https://ffmpeg.org/download.html) |
| `lewton` | 0.10.2 | Vorbis, in process only | A Rust crate that cargo fetches by itself as a test dependency ([github.com/RustAudio/lewton](https://github.com/RustAudio/lewton)) |

On macOS with Homebrew, one line installs everything that is not built in:

```sh
brew install flac mpg123 lame opus-tools ffmpeg
```

These tools are for speed. Correctness is checked against the official
conformance files instead (`just corpus` fetches them; the README's Tests
section says which).

## Repeating it

```sh
scripts/compare-with-flac.py      # FLAC encode and decode against flac
scripts/compare-decoders.py       # MP3, AAC, ALAC and Opus decode against mpg123, afconvert and opusdec
scripts/compare-encoders.py       # Apple Lossless and Opus encode against afconvert, ffmpeg and opusenc
```

Each script builds the release binary, makes its own test signal, encodes the
test files with the reference tools, times both sides and prints the table.
They need Python 3 and the tools above; a tool that is missing leaves its
column out. To measure your own audio, pass a 16-bit WAV:

```sh
scripts/compare-with-flac.py --reps 11 song.wav
scripts/compare-decoders.py song.wav
scripts/compare-encoders.py --reps 11 song.wav   # a 16- or 24-bit WAV
```

The AAC rows need a build with the `aac` feature, which the script turns on
(see the README: AAC decoding is off in a default build).

## Reading the results

- **One machine, one signal per row.** Expect a few percent of run-to-run
  noise. The FLAC and resampling rows were taken on a quiet machine, the
  decoder rows with other work running. The Linux rows were taken on a
  shared cloud VM with nothing else of ours running; its speed drifted by up
  to 10% from one series to the next, so read the ratios, which held (Apple
  Lossless 3.5 to 3.7 times slower on the 16-bit music in four series), not the
  milliseconds.
- **Threads.** FLAC, ALAC and the resampler use several threads by default;
  MP3 and AAC decode on one, which is why their two columns match.
- **What sets the pace.** FLAC is held by the MD5 of the audio, which the
  format requires and which cannot be split across threads. For MP3 and AAC,
  the dithered conversion to 16-bit is about a fifth of the time.
- **What the encoders do with their time.** Measured with user and system CPU
  time on the 256 s music file: Apple Lossless takes 6.0 s of user time in a 6.0 s
  run (one busy thread), `ffmpeg` 1.35 s of user and 0.34 s of system time in 1.75 s.
  Reading and writing are not the cost: `crossfade convert` of the same WAV to a WAV
  takes 18 ms. For Opus at 128 kbit/s Crossfade used 5.4 s of user time in a 4.2 s run
  (5.0 s in 5.0 s with `--threads 1`) and `opusenc` 3.6 s in 3.7 s. Nothing here
  says which part of either encoder is the slow one; that has not been profiled.
- **Not measured.** Windows, other x86 machines and Linux on ARM; `afconvert`
  against Apple Lossless encode and Apple's Opus (macOS only, to do); Vorbis
  against libvorbis (no tool installed); Opus encode at other bitrates, frame
  sizes and in CBR.

Everything is safe Rust: every crate forbids `unsafe`, so there are no
hand-written SIMD intrinsics.
