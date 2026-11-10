# Ogg FLAC fixtures

Each `.oga` is made by the reference encoder (flac 1.5.0) from raw PCM written by
`cargo run -p crossfade-format --example ogg_fixtures -- <dir>`. Each `.flac` twin is the same
audio and settings without `--ogg`: the encoder writes identical frames in both containers, and
the tests compare them. Serial numbers are fixed so the files are reproducible.

```sh
R16="--force-raw-format --endian=little --sign=signed --channels=2 --bps=16 --sample-rate=44100"
M24="--force-raw-format --endian=little --sign=signed --channels=1 --bps=24 --sample-rate=48000"
N8="--force-raw-format --endian=little --sign=signed --channels=1 --bps=8 --sample-rate=44100"
flac -s -f --no-padding --ogg --serial-number=1001 $R16 -o stereo16.oga stereo16.raw
flac -s -f --no-padding $R16 -o stereo16.flac stereo16.raw
flac -s -f --no-padding --ogg --serial-number=2002 $M24 -o mono24.oga mono24.raw
flac -s -f --no-padding $M24 -o mono24.flac mono24.raw
flac -s -f --no-padding --ogg --serial-number=3003 -T TITLE=Crossfade -T ARTIST=Tester -T ALBUM=Fixtures $R16 -o tagged.oga stereo16.raw
flac -s -f --no-padding -T TITLE=Crossfade -T ARTIST=Tester -T ALBUM=Fixtures $R16 -o tagged.flac stereo16.raw
flac -s -f --no-padding --ogg --serial-number=4004 -b 256 $R16 -o smallblock.oga stereo16.raw
flac -s -f --no-padding -b 256 $R16 -o smallblock.flac stereo16.raw
flac -s -f --no-padding --ogg --serial-number=5005 --lax -b 8192 $N8 -o noise8.oga noise8.raw
flac -s -f --no-padding --lax -b 8192 $N8 -o noise8.flac noise8.raw
flac -s -f --no-padding --ogg --serial-number=6006 $R16 -o serial.oga stereo16.raw
```

| File | Exercises |
|---|---|
| `stereo16` | 16-bit stereo, default block size: one page holds every frame |
| `mono24` | 24-bit mono at 48 kHz |
| `tagged` | a VORBIS_COMMENT header packet with three tags |
| `smallblock` | 256-sample blocks: many packets per page, several pages |
| `noise8` | white noise with 8192-sample blocks: 8 KB packets |
| `serial` | the `stereo16` audio under another serial number (`--serial-number`); multiplexed with `stereo16` by tests |

The encoder never splits a packet across pages at these sizes (that needs a packet over 64 KB),
so the tests re-page `noise8.oga` themselves for the continued-packet cases.
