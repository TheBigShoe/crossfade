# MP3 fixtures

Every `.mp3` here is output of `lame` (LAME 64bits version 4.0) for a signal that
`tests/support/mp3_signals.rs` generates (testkit's sweep and sine, rounded to 16 bits), so the
tests regenerate the source and check the decoder's gapless length and alignment against it.
`regen.sh` re-creates all of them:

```sh
tests/fixtures/mp3/regen.sh /tmp/mp3src   # writes the WAV sources, then runs lame on each
```

which is `lame --quiet <options> <name>.wav <name>.mp3` with:

| File | Source | Options | Exercises |
|---|---|---|---|
| `cbr128.mp3` | 44.1 kHz stereo, 0.5 s | `-b 128` | MPEG-1 CBR with LAME's `Info` frame (delay 576), joint stereo |
| `cbr320.mp3` | 44.1 kHz stereo, 0.5 s | `-b 320` | the highest MPEG-1 bitrate |
| `vbr.mp3` | 44.1 kHz stereo, 0.5 s | `-V 2` | VBR with a `Xing` frame |
| `notag.mp3` | 44.1 kHz stereo, 0.5 s | `-t -b 128` | no info frame: only the decoder's delay is known |
| `mpeg1-32k.mp3` | 32 kHz mono, 1 s | `-b 32 -m m --resample 32` | the lowest MPEG-1 bitrate; frames too small for an info frame |
| `mpeg2.mp3` | 22.05 kHz stereo, 1 s | `-b 64 --resample 22.05` | MPEG-2 (LSF) with an info frame |
| `mpeg2-8k.mp3` | 16 kHz mono, 1 s | `-b 8 -m m --resample 16` | the lowest MPEG-2 bitrate |
| `mpeg25.mp3` | 8 kHz mono, 1 s | `-b 8 -m m --resample 8` | MPEG-2.5, its lowest bitrate |
| `free.mp3` | 44.1 kHz stereo, 0.5 s | `--freeformat -b 200` | free format (bitrate index 0) |
| `tagged.mp3` | 44.1 kHz stereo, 0.5 s | `-b 128 --add-id3v2 --id3v2-utf16 --tt Title --ta Artist --tl Album --ty 2024 --tn 3/9 --tg Rock --tc comment` | an ID3v2.3 tag in UTF-16 with a BOM, and an ID3v1.1 tag at the end |

LAME 4.0 on macOS mangles non-ASCII text given on the command line, so the tags are ASCII; the
reader's unit tests cover non-ASCII text in every ID3v2 encoding, and `mp3_fixtures.rs` puts an
ID3v2.4 tag with UTF-16 text in front of `cbr128.mp3`, and junk in front of it, in the test.

The `.mpg123.f32` files are short references from `mpg123 1.33.7`: 576 sample frames (raw
little-endian `f32`, interleaved) from the start of the third audio frame of the decoder output,
without gapless trimming:

```sh
mpg123 -q -e f32 --no-gapless -s cbr128.mp3 | dd bs=1 skip=18432 count=4608 of=cbr128.mpg123.f32
mpg123 -q -e f32 --no-gapless -s mpeg2.mp3  | dd bs=1 skip=9216 count=4608 of=mpeg2.mpg123.f32
mpg123 -q -e f32 --no-gapless -s mpeg25.mp3 | dd bs=1 skip=4608 count=2304 of=mpeg25.mpg123.f32
```
