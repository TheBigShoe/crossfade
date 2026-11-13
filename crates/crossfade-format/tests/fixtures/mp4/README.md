# MP4 fixtures

Every `.m4a` here is output of macOS `afconvert` (Audio File Convert 2.0, macOS 27) for a
signal that `tests/support/mp4_signals.rs` generates with integer arithmetic only, so
`tests/mp4_fixtures.rs` rebuilds the exact samples and compares the decoded ALAC to them.
`regen.sh` re-creates all of them:

```sh
cargo run -p crossfade-format --example mp4_fixtures -- /tmp/mp4src   # the source WAVs
tests/fixtures/mp4/regen.sh /tmp/mp4src
```

The source WAVs are `WAVE_FORMAT_EXTENSIBLE` with the real depth in `wValidBitsPerSample` and
the mask of ALAC's layout for the channel count (5.0 `0x607`, 5.1 `0x60F`, 6.1 `0x70F`, 7.1
`0x6CF`: `afconvert` takes WAVE's side pair as Core Audio's Ls/Rs, which ALAC's layouts use).
Each ALAC file is `afconvert --no-filler -f m4af -d alac <src>.wav <out>.m4a` (20-bit:
`-d alac/2`, since `-d alac` keeps 16, 24 or 32 bits from the source); the AAC one is
`afconvert --no-filler -f m4af -d aac stereo16.wav aac-stereo16.m4a`. `afconvert` writes
`ftyp`, `moov`, `free` (padding to 4096 even with `--no-filler`), `mdat`, no edit list, and an
`iTunSMPB` tag; the test makes the `moov`-last form by moving boxes.

| File | Signal | `afinfo` | Exercises |
|---|---|---|---|
| `alac-mono16.m4a` | `mono16` | 1 ch, 22050 Hz, 16-bit, 5001 frames | mono (SCE) |
| `alac-stereo16.m4a` | `stereo16` | 2 ch, 44100 Hz, 16-bit, 10007 frames in 3 packets | CPE, mid-side, a partial last packet, seeking |
| `alac-stereo20.m4a` | `stereo20` | 2 ch, 48000 Hz, 20-bit | 20 bits in `S24` |
| `alac-stereo24.m4a` | `stereo24` | 2 ch, 96000 Hz, 24-bit | shifted bytes, a rate the entry's 16.16 field cannot hold |
| `alac-stereo32.m4a` | `stereo32` | 2 ch, 48000 Hz, 32-bit | 32 bits, shifted bytes |
| `alac-ch3.m4a` .. `alac-ch7.m4a` | `ch3` .. `ch7` | 3.0, 4.0, 5.0, 5.1, 6.1 (C L R ...), 16-bit | SCE/CPE/LFE sequences, channel order |
| `alac-ch8.m4a` | `ch8` | 7.1 (C Lc Rc L R Ls Rs LFE), 24-bit | eight channels |
| `alac-noise16.m4a` | `noise16` | 2 ch, full-scale white noise | escape (uncompressed) frames |
| `alac-silence.m4a` | `silence` | 1 ch, 9000 zeros | zero runs |
| `aac-stereo16.m4a` | `stereo16` | see below | the reader only: no AAC decoder here |

`afinfo aac-stereo16.m4a`:

```
Data format:     2 ch,  44100 Hz, aac  (0x00000000) 0 bits/channel, 0 bytes/packet, 1024 frames/packet, 0 bytes/frame
estimated duration: 0.226916 sec
audio bytes: 3141
audio packets: 12
audio 10007 valid frames + 2112 priming + 169 remainder = 12288
Channel layout: Stereo (L R)
```

Its `esds` holds the AudioSpecificConfig `12 10` (AAC LC, 44.1 kHz, channel configuration 2).
