# Crossfade

An audio (and, later, video) conversion tool in pure Rust: one engine, a
command line (`crossfade`) and a desktop app (`crossfade-gui`).

Crossfade is early. It handles lossless audio today; lossy codecs, DSP and
video are planned. It has only been run on macOS so far.

## What works

| Format | Read | Write | Notes |
|---|---|---|---|
| WAV | yes | yes | 8- to 32-bit integer, 32- and 64-bit float; `LIST/INFO` tags |
| FLAC | yes | yes | Our own decoder, checked against the IETF FLAC test files. Output is limited to 96 kHz and 24-bit for now |
| AIFF / AIFF-C | yes | yes | Integer and float PCM; text-chunk tags |
| Ogg | FLAC only | no | Vorbis and Opus are recognised and refused by name |

Conversions between these are bit-exact. Tags are carried across where the
target container can hold them, and `--codec copy` moves audio from one
container to another without decoding it.

## Command line

```sh
crossfade probe song.flac            # what the file contains
crossfade probe song.flac --json     # the same, machine-readable
crossfade convert song.wav -o song.flac
crossfade convert song.wav -o song.flac --compression-level 8
crossfade convert song.flac -o song.aiff --sample-format s24
crossfade convert song.oga -o song.flac --codec copy
```

`crossfade convert --help` lists every option. Output is written to a
temporary file and moved into place only when the conversion succeeds. An
existing output is never replaced without `--force`.

## Desktop app

```sh
just run
```

Open or drop files to inspect them, convert with the same options the command
line has, and watch a queue with progress, cancel and retry. Every queued job
can show the equivalent `crossfade` command. Settings, themes and key bindings
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
just corpus   # once per clone: fetch the FLAC conformance files (about 300 MB)
just ci       # formatting, lints, tests, and the conformance tier
```

Plain `cargo test --workspace` needs no network, no audio device, no display
and no other tool. The conformance tier decodes the
[IETF FLAC decoder test files](https://github.com/ietf-wg-cellar/flac-test-files)
and compares the results with a committed record.

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
