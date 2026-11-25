# Crossfade

An audio (and, later, video) conversion tool in pure Rust: one engine, a
command line (`crossfade`) and a desktop app (`crossfade-gui`).

Crossfade is early. It writes WAV, FLAC, AIFF, Ogg FLAC, Apple Lossless in M4A and Opus, and
reads those and more lossy audio (MP3, AAC, Vorbis), with
processing: sample rate and channel conversion, gain, loudness normalising,
trim, fades and joining files. More lossy encoders (AAC, MP3, Vorbis) and video are planned. It is
developed on macOS; its tests and conformance tier also pass on Linux x86-64, where the
desktop app has been run on a virtual display only. Windows is untried.

## What works

| Format | Read | Write | Notes |
|---|---|---|---|
| WAV | yes | yes | 8- to 32-bit integer, 32- and 64-bit float; `LIST/INFO` tags; true depth below the container (valid bits) and speaker layout (channel mask) |
| FLAC | yes | yes | Our own decoder, checked against the IETF FLAC test files. Output is 4- to 32-bit integer, up to 8 channels, any rate up to 1,048,575 Hz; speaker layouts other than FLAC's defaults travel in the channel mask tag |
| AIFF / AIFF-C | yes | yes | Integer and float PCM; text-chunk tags; true depth as `sampleSize`. No channel mask: layouts beyond mono and stereo are not kept |
| Ogg | FLAC, Vorbis and Opus | FLAC; Opus (`--codec opus`, or the name `.opus`); an Opus stream by `--codec copy` | `.oga`, `.ogg` and `.opus`: gapless from the granule positions (priming, Opus's pre-skip and end trimming cut), seeking, Vorbis comments as tags; a chained file is read to the end of its first link. `.oga` and `.ogg` hold FLAC unless `--codec opus`; `.opus` holds Opus only |
| Opus | yes (in Ogg) | yes (in Ogg), 1 to 8 channels | Our own decoder (SILK, CELT and hybrid, every mode switch, packet loss concealment, multistream up to 8 channels in WAVE speaker order; ambisonics refused), the same bits on every platform: all twelve RFC 8251 test vectors pass, the SILK-only ones sample for sample. Always 48 kHz 32-bit float (16-bit by default to an integer container), the header's output gain applied. Seeking starts decoding 1 s early, so the audio just after a seek is within 0.001 (about 30 16-bit steps) of a decode from the start, often exact, not always |. Our own encoder (20 ms frames, variable bitrate by default or constant with `--cbr`; `--bitrate` 6 to 256 kb/s per channel, 96 kb/s mono, 128 kb/s stereo and 320 kb/s for 5.1 when not given; mapping family 1 for 3 to 8 channels): input is resampled to 48 kHz with a note, a header records the source's rate, the pre-skip is 312 and the length is exact; files pass `opusinfo`, decode in `opusdec` and FFmpeg to the same length, and in our decoder. A rate other than 48 kHz and a sample format are refused (Opus has no bit depth); three or more channels are written as Vorbis's layout for their count (3.0, quad, 5.0-back, 5.1-back, 6.1, 7.1): the same speakers with the surround pair named side (quad, 5.0 and 5.1 with side surrounds, the mask 0x60F many tools write) and channels with no layout at all are written channel for channel, with a note, and read back as Vorbis's layout (5.1 with back surrounds); another layout (4.0, a different centre or LFE set) is refused with `--channels stereo` or a matrix as the advice |
| Vorbis | yes (in Ogg) | no | Our own decoder, the same bits on every platform: Xiph's 29 test files decode, within 9e-6 of lewton. Output is 32-bit float (16-bit by default when written to an integer container), 1 to 8 channels in WAVE speaker order |
| MP4 / M4A | yes | ALAC only | `.mp4`, `.m4a`, `.m4b`: the first audio track, gapless (edit list or `iTunSMPB`), iTunes tags, seeking; ALAC tracks decode, AAC-LC tracks decode with `--features aac`, MP3 tracks decode; `moov` after `mdat` needs a seekable input; fragmented and encrypted files are refused. Writing needs an output that can seek, carries the tags and an edit list for the exact length, and takes `--codec copy` from another ALAC `.m4a`; AAC and MP3 are not written |
| ALAC | yes (in MP4) | yes (in MP4) | Our own decoder, bit-exact against `afconvert`: 16-, 20-, 24- and 32-bit, 1 to 8 channels in WAVE speaker order. Our own encoder (any depth and channel count, mid-side and predictor search, never larger than raw; one thread, the same bytes every time); its files decode bit for bit in our decoder and in FFmpeg's. ALAC stores one layout per channel count (mono, stereo, 3.0, 4.0, 5.0, 5.1, 6.1 and an 8-channel one no `--channels` name selects): the same speakers with the surround pair named back (5.0 and 5.1 with back surrounds, FLAC's default for six channels) and channels with no layout at all are written channel for channel, bit for bit, with a note, and read back as ALAC's layout (side surrounds); another layout (quad, which is not ALAC's 4.0, or 7.1, which is not its 8-channel layout) is refused with a note on what would work. A depth ALAC has no name for is written one up (12-bit as 16-bit) and the plan says so |
| MP3 | yes | no | MPEG-1, 2 and 2.5 Layer III with our own decoder, full accuracy on the ISO conformance streams; gapless from LAME's info frame, exact seeking, ID3v2 and ID3v1 tags. Layers I and II are named and refused |
| ADTS (`.aac`) | AAC-LC, with `--features aac` | no | Our own AAC-LC decoder, checked against the ISO conformance streams and Apple's decoder; mono to 7.1 and program configurations, 8 to 96 kHz, seeking. HE-AAC plays as its AAC-LC core; Main, LTP, SSR, coupling channels and 960-sample frames are refused by name. A default build refuses AAC and says why |

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
never changes a byte. MP3, AAC and ALAC decode faster than `mpg123` and macOS's
`afconvert` too. Opus decodes about twice as fast as `opusdec`. The Apple Lossless and
Opus encoders are not faster than their references: on a Linux x86-64 machine
`ffmpeg` encodes Apple Lossless 3.6 to 6 times faster, and `opusenc` encodes Opus 9 to
23% faster, at about the same file size. [docs/performance.md](docs/performance.md) has every result on
one page, what the reference tools are and how to get them, and the scripts
that repeat the measurement; the full tables are in
[docs/performance-details.md](docs/performance-details.md).

## Command line

```sh
crossfade probe song.flac            # what the file contains
crossfade probe song.flac --json     # the same, machine-readable
crossfade convert song.wav -o song.flac
crossfade convert song.wav -o song.flac --compression-level 8
crossfade convert song.wav -o song.flac --threads 1   # one thread (the default is auto)
crossfade convert song.flac -o song.aiff --sample-format s24
crossfade convert song.oga -o song.flac --codec copy
crossfade convert song.wav -o song.m4a                 # Apple Lossless
crossfade convert song.m4a -o copy.m4a --codec copy    # the packets, unchanged
crossfade convert song.wav -o song.opus --bitrate 128k # Ogg Opus (48 kHz; other rates are resampled)
crossfade convert song.wav -o song.opus --bitrate 64kbps --cbr
crossfade convert song.wav -o song.ogg --codec opus    # Opus in .ogg (the default there is FLAC)
crossfade convert song.opus -o song.ogg --codec copy   # an Opus stream into Ogg, undecoded
crossfade convert song.mp3 -o song.flac
crossfade convert song.opus -o song.flac
crossfade probe song.m4a
```

Readable: WAV, FLAC, AIFF/AIFF-C, Ogg (FLAC, Vorbis and Opus), MP3, MP4/M4A (ALAC;
AAC too, as is ADTS `.aac`, in a build with the `aac` feature). Writable: WAV,
FLAC, AIFF, Ogg FLAC, M4A (Apple Lossless), Opus (`.opus`, or `--codec opus` into `.ogg` or
`.oga`), and with `--codec copy` an Opus stream into Ogg. `crossfade convert --help` lists every option. FLAC decoding and encoding use
several threads unless `--threads 1` says otherwise; the output is the same
either way. Output is written to a
temporary file and moved into place only when the conversion succeeds. An
existing output is never replaced without `--force`.

## Processing

Filters run in the order given, one per `--filter`: a name, an optional value,
then `key=value` words. Units are always written (`dB`, `s`, `ms`, `1:30`,
`LUFS`, `dBTP`, `Hz`, `kHz`, `128k` or `64kbps` for a bitrate), and a mistake names the word and what was expected.

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
| resample | `resample 48kHz quality=best` (`fast`, `good`, `best`); also `--sample-rate`; Opus output adds one to 48 kHz by itself, with a note | Band-limited, time-aligned, on several threads with the same bytes; a 44.1 -> 48 -> 44.1 kHz round trip keeps 143 / 159 / 239 dB (fast / good / best) |
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
double-click to fit), play them through the default output device (Space plays
and pauses, Home and End jump, a click on the waveform seeks and a drag scrubs;
the playhead follows playback until you scroll), convert with the same options the command
line has (sample rate, channels, and an Adjust section for gain or loudness
normalise, trim and fades), and watch a queue with progress, cancel and retry.
A normalising job shows Measuring first; a finished job says how many samples
clipped, or that the true-peak ceiling kept it below its loudness target. Every
queued job can show the equivalent `crossfade` command. Settings, themes and key bindings
are plain files in the config folder (`~/.config/crossfade` on macOS and
Linux) and reload when saved.

## iPhone and iPad app

A native SwiftUI app for iPhone and iPad lives in `apps/ios`, driving the same app layer
through `crates/crossfade-ffi`. **It has never been built or run**: it was written on a
machine without Xcode. `apps/ios/README.md` says how to build it on a Mac (`just
ios-framework`, `just ios-run`) and what is most likely to need fixing first.

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
the Xiph Vorbis, MP3, AAC (ISO/IEC 14496-26 and 13818-4) and Opus (RFC 8251) test
vectors, and compares the results with a committed record. None is committed: `corpus.toml`
only lists where each one lives. `just corpus` takes them all; to fetch one codec, run
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

MIT or Apache-2.0, at your option. Patent-encumbered codecs are source-only
behind off-by-default features and never part of release binaries: AAC
decoding needs `cargo build -p crossfade-cli --features aac`.
