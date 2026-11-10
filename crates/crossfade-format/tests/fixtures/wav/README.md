# WAV fixtures

Multichannel float WAV written by Apple's `afconvert` (macOS), the independent oracle for the
`WAVE_FORMAT_EXTENSIBLE` float header. `tests/wav_fixtures.rs` regenerates the expected samples
and checks the layout. Each file also carries the `FLLR` filler chunk `afconvert` adds, which
the reader skips.

Source: `make_source.py` writes a 16-bit extensible WAV (32 frames, 48000 Hz,
`sample(frame, channel) = frame * 37 + channel * 1000 - 3000`).

```sh
python3 make_source.py 6 1551 s6.wav   # mask 0x60F: FL FR FC LFE SL SR
python3 make_source.py 4 51 s4.wav     # mask 0x33: FL FR BL BR
afconvert -f WAVE -d LEF32 -l MPEG_5_1_A s6.wav f32-5.1.wav
afconvert -f WAVE -d LEF64 -l MPEG_5_1_A s6.wav f64-5.1.wav
afconvert -f WAVE -d LEF32 s4.wav f32-quad.wav
```

What `afconvert` wrote (read from a hex dump): format tag `0xFFFE`, `cbSize` 22, valid bits equal
to the sample width, masks `0x60F` (5.1) and `0x33` (quad), the IEEE-float sub-format GUID,
and no `fact` chunk. Stereo and mono float it writes as a plain 16-byte tag-3 `fmt `. The quad
file is made without `-l`: `-l Quadraphonic` made `afconvert` drop the layout and write a
plain tag-3 header with no mask.
