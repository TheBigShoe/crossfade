# Crossfade

An audio (and, later, video) conversion tool in pure Rust: one engine, a
command line (`crossfade`) and a desktop app (`crossfade-gui`).

Crossfade is early. It handles lossless audio today, with processing: sample
rate and channel conversion, gain, loudness normalising, trim, fades and
joining files. Lossy codecs and video are planned. It has only been run on
macOS so far.

## What works

| Format | Read | Write | Notes |
|---|---|---|---|
| WAV | yes | yes | 8- to 32-bit integer, 32- and 64-bit float; `LIST/INFO` tags; true depth below the container (valid bits) and speaker layout (channel mask) |
| FLAC | yes | yes | Our own decoder, checked against the IETF FLAC test files. Output is 4- to 32-bit integer, up to 8 channels, any rate up to 1,048,575 Hz; speaker layouts other than FLAC's defaults travel in the channel mask tag |
| AIFF / AIFF-C | yes | yes | Integer and float PCM; text-chunk tags; true depth as `sampleSize`. No channel mask: layouts beyond mono and stereo are not kept |
| Ogg | FLAC only | FLAC only | `.oga` and `.ogg`; Vorbis and Opus are recognised and refused by name |
| MP4 / M4A | yes | no | `.mp4`, `.m4a`, `.m4b`: the first audio track, gapless (edit list or `iTunSMPB`), iTunes tags, seeking; ALAC tracks decode, AAC and MP3 tracks are read but not decoded yet; `moov` after `mdat` needs a seekable input; fragmented and encrypted files are refused |
| ALAC | yes (in MP4) | no | Our own decoder, bit-exact against `afconvert`: 16-, 20-, 24- and 32-bit, 1 to 8 channels in WAVE speaker order |

Conversions between these are bit-exact, and a file's true bit depth (a 20-bit FLAC stays 20-bit, with the same STREAMINFO MD5) survives them unless `--sample-format` changes the format. Tags are carried across where the
target container can hold them, and `--codec copy` moves audio from one
container to another without decoding it.

## Speed

Measured against the reference `flac` 1.5.0 on an Apple M1 Max, whole process
against whole process, best of 11 runs (milliseconds):

| 60 s of 16-bit 44.1 kHz stereo | `crossfade` | `crossfade --threads 1` | `flac` | `flac -j8` |
|---|---|---|---|---|
| Encode, level 0 | 22.5 | 46.0 | 62.0 | 26.2 |
| Encode, level 5 | 22.5 | 75.6 | 100.7 | 24.4 |
| Encode, level 8 | 23.9 | 139.3 | 205.0 | 33.3 |
| Decode (level 5 file) | 21.4 | 45.8 | 70.3 | |

Files are the same size as the reference's or smaller, and the thread count
never changes a byte. [docs/performance.md](docs/performance.md) has the full
tables, the method, the caveats and the script that repeats the measurement.

## Command line

```sh
crossfade probe song.flac            # what the file contains
crossfade probe song.flac --json     # the same, machine-readable
crossfade convert song.wav -o song.flac
crossfade convert song.wav -o song.flac --compression-level 8
crossfade convert song.wav -o song.flac --threads 1   # one thread (the default is auto)
crossfade convert song.flac -o song.aiff --sample-format s24
crossfade convert song.oga -o song.flac --codec copy
```

`crossfade convert --help` lists every option. FLAC decoding and encoding use
several threads unless `--threads 1` says otherwise; the output is the same
either way. Output is written to a
temporary file and moved into place only when the conversion succeeds. An
existing output is never replaced without `--force`.

## Processing

Filters run in the order given, one per `--filter`: a name, an optional value,
then `key=value` words. Units are always written (`dB`, `s`, `ms`, `1:30`,
`LUFS`, `dBTP`, `Hz`, `kHz`), and a mistake names the word and what was expected.

```sh
crossfade convert in.wav -o out.flac --gain -6dB                 # shortcut for --filter "gain -6dB"
crossfade convert in.wav -o out.flac --filter "gain -6dB" --filter "fade out=3s curve=equal-power"
crossfade convert in.wav -o out.flac --start 10s --end 1:30      # one trim filter
crossfade convert in.wav -o out.flac --normalize -16LUFS --true-peak -1dBTP
crossfade convert in.wav -o out.flac --sample-rate 48000 --channels stereo
crossfade concat a.wav b.wav -o both.flac                        # several inputs: a separate command
```

| Filter | Example | Notes |
|---|---|---|
| gain | `gain -6dB` | -120 to +60 dB |
| trim | `trim start=10s end=1:30`, `trim duration=500ms` | Sample-accurate |
| fade | `fade in=2s out=3s curve=equal-power` (`linear`, `equal-power`, `s-curve`) | A fade-out holds its length in memory: at most 10 minutes |
| normalize | `normalize -16LUFS peak=-1dBTP` (two passes: measure, then convert) | Measures the signal as it reaches the stage (after any trim or rate change before it); a note says when the true-peak ceiling stops it short of the target |
| resample | `resample 48kHz quality=best` (`fast`, `good`, `best`); also `--sample-rate` | Band-limited, time-aligned, on several threads with the same bytes; a 44.1 -> 48 -> 44.1 kHz round trip keeps 143 / 159 / 239 dB (fast / good / best) |
| mix | `mix stereo`, `mix 1 matrix=0.5,0.5`; also `--channels` | Standard down- and up-mixes by speaker position (BS.775, LFE dropped, level-normalised), or a matrix; unlabelled channels mixed down are taken as FLAC's default layout for their count, with a note |
| concat | `crossfade concat a.wav b.wav -o out.flac` | Inputs conformed to the first one's rate and layout, sample-exact seams, bit-exact when the inputs match |

A trim stops reading the source once past its end. A job
with no filters, no rate or channel change and no loss of depth takes exactly
the path it took before, bit for bit. When something is processed, or an
integer output is narrower than the input's real bit depth (24 real bits to
`s16`, float to integer), integer output gets TPDF dither (`--dither none`
turns it off and keeps the old bytes; float output is never dithered) from a
fixed-seed generator, so the same input gives the same bytes on every machine
and at every thread count. Samples pushed past full scale are clipped and
counted, and the command line prints a warning with the count.

## Desktop app

```sh
just run
```

Open or drop files to inspect them (a zoomable waveform and a loudness readout
appear under the file name; scroll to pan, pinch or Cmd/Ctrl-scroll to zoom,
double-click to fit), convert with the same options the command
line has (sample rate, channels, and an Adjust section for gain or loudness
normalise, trim and fades), and watch a queue with progress, cancel and retry.
A normalising job shows Measuring first; a finished job says how many samples
clipped, or that the true-peak ceiling kept it below its loudness target. Every
queued job can show the equivalent `crossfade` command. Settings, themes and key bindings
are plain files in the config folder (`~/.config/crossfade` on macOS and
Linux) and reload when saved.

## Building

Rust 1.98.1 (pinned in `rust-toolchain.toml`) and [`just`](https://github.com/casey/just).
The desktop app is built on GPUI; on macOS that needs full Xcode for the Metal
shader compiler.

```sh
cargo build --release -p crossfade-cli   # the command line only
cargo build --release -p crossfade-ui    # the desktop app
```

## Tests

```sh
just corpus   # once per clone: fetch the conformance files (about 550 MB; ~600 MB on disk)
just ci       # formatting, lints, tests, and the conformance tier
```

Plain `cargo test --workspace` needs no network, no audio device, no display
and no other tool. The conformance tier decodes the
[IETF FLAC decoder test files](https://github.com/ietf-wg-cellar/flac-test-files)
and compares the results with a committed record. The corpus also holds the
official Opus (RFC 8251), Vorbis (Xiph), MP3 and AAC (ISO/IEC 14496-26) test
vectors for the decoders to come; none is committed, `corpus.toml` only lists
where each one lives. `just corpus` takes them all; to fetch one codec, run
`cargo run --release -p crossfade-corpus -- fetch --only flac/` (the flag
repeats, and `verify`, `list` and `seed` take it too).

## Design

- **Pure Rust engine.** The engine crates link no C or assembly, checked by a
  test, and every crate forbids `unsafe`.
- **Test first.** Each behaviour starts as a failing test; the history shows
  the red commit before the green one.
- **One job model.** The command line and the app build the same job
  description, and tests hold them to it.
- **Hostile input is expected.** Readers never trust a length from a file, and
  each reader and decoder has a fuzz target.

`CLAUDE.md` describes the crate layout and the project's working rules.

## Licence

MIT or Apache-2.0, at your option. Patent-encumbered codecs, when they
arrive, will be source-only behind off-by-default features and never part of
release binaries.
