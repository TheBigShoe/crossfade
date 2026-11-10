# FLAC fixtures

Every `.flac` here is output of the reference encoder (`flac 1.5.0`) for a signal that
`tests/support/signals.rs` generates with integer arithmetic only, so the tests rebuild the
exact samples and compare the decoder's output to them. `regen.sh` re-creates all of them:

```sh
cargo run -p crossfade-format --example flac_fixtures -- /tmp/raw   # raw PCM and WAV inputs
tests/fixtures/flac/regen.sh /tmp/raw
```

All use `--no-padding` unless stated. `enc` below is
`flac -s -f --no-padding --force-raw-format --endian=little --sign=signed --sample-rate=R --channels=C --bps=B`
on the signal's `.raw` file (12 and 20 bit go in as WAVE_FORMAT_EXTENSIBLE `.wav`, since raw
input only takes 8/16/24/32 bits).

| File | Signal | Options | Exercises |
|---|---|---|---|
| `mono-8bit.flac` | 8 kHz mono 8 bit tones | default | 8 bit (`U8` output), mono, LPC |
| `stereo-12bit.flac` | 44.1 kHz 12 bit | WAV input | 12 bit left-justified into `S16` |
| `stereo-16bit.flac` | 44.1 kHz 16 bit | default | the plain case |
| `stereo-20bit.flac` | 48 kHz 20 bit | WAV input | 20 bit into `S24` |
| `stereo-24bit.flac` | 96 kHz 24 bit | default | 24 bit, 5-bit Rice parameters, wasted bits from the shift |
| `stereo-32bit.flac` | 48 kHz 32 bit, full-scale opposite samples | default | 32 bit, mid/side with a 33-bit side channel, verbatim and fixed subframes |
| `surround-5.1.flac` | 48 kHz 16 bit, 6 channels | default | six independent channels |
| `fixed-only.flac` | 48 kHz mono 24 bit, blocks built for each fixed order | `-l 0 -b 192 -r 4` | fixed predictors of order 0 to 4, both Rice parameter widths, partition orders 0 and 1 |
| `lpc-order-32.flac` | `stereo16` | `--lax -l 32 -b 4096` | LPC orders up to 29 in practice (the encoder's choice) |
| `verbatim-noise.flac` | full-range white noise | default | verbatim subframes |
| `constant-silence.flac` | zeros, mono | `-b 4608` | constant subframes, block size 4608 |
| `wasted-bits.flac` | tones with 3 and 5 zero low bits | default | wasted bits |
| `stereo-modes-m.flac` | four stretches of noise, one per mode | `-m -b 1024 -l 0` | left/side, right/side and mid/side frames |
| `stereo-modes-adaptive.flac` | same | `-M -b 1024 -l 0` | independent and mid/side frames |
| `stereo-modes-none.flac` | same | `--no-mid-side -b 1024 -l 0` | independent frames only |
| `block-16.flac` | `stereo16` | `--lax -b 16` | the smallest block size (many tiny frames) |
| `block-4608.flac` | `stereo16` | `-b 4608` | block size code 5 |
| `block-1000.flac` | `stereo16` | `-b 1000` | an odd block size (header carries it as 8 bits) |
| `block-65535.flac` | `stereo16` | `--lax -b 65535` | the largest block size, one frame |
| `tagged.flac` | `stereo16` | `--padding=300 -S 4x -T TITLE=Fixture -T ARTIST=Crossfade --picture=...pixel.gif` | VORBIS_COMMENT, SEEKTABLE, PICTURE and PADDING blocks |
| `id3v2-prefixed.flac` | `stereo16` | `stereo-16bit.flac` with a hand-made 21-byte ID3v2.4 tag in front | tag skipped by its syncsafe size |
| `unknown-total.flac` | `stereo16` | piped to `flac -c -` | total samples 0 and MD5 all zero (the encoder cannot rewrite STREAMINFO on a pipe) |

Stereo modes, as counted by `flac_check --modes`: `stereo-modes-m` holds 2 left/side, 2
right/side and 4 mid/side frames; `stereo-modes-adaptive` 2 independent and 6 mid/side;
`stereo-modes-none` 8 independent; `stereo-32bit` one mid/side frame. The test
`the_fixtures_between_them_use_every_stereo_mode_and_the_channel_counts` keeps that true.

The reference encoder never writes an escaped Rice partition; the unit tests in
`crossfade-codec` build those by hand, and the conformance files `subset/16`, `subset/32` and
`subset/64` of the IETF decoder test files carry them (run `flac_check` over them).
