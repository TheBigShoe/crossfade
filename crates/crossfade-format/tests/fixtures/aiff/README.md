# AIFF fixtures

Written by Apple's `afconvert` (macOS) from WAV files that
`cargo run -p crossfade-format --example aiff_fixtures -- sources DIR` produces. The samples
come from `tests/support/signal.rs` (600 stereo frames at 22050 Hz: a 440 Hz and a 1 kHz sine);
`tests/aiff_fixtures.rs` regenerates them and checks every file decodes to exactly that.
`afconvert` also writes an `FLLR` filler chunk before `SSND`, which the reader must skip.

Source WAV in `DIR`: `u8.wav s16.wav s24.wav s32.wav f32.wav f64.wav`, and `s8src.wav`: the
8-bit signal as 16-bit samples (`(u8 - 128) * 256`). The signed 8-bit files come from it
because `afconvert -d I8` on an unsigned 8-bit WAV copies the bytes without converting them
(silence lands as 0x80, i.e. -128), which would make a wrong fixture.

| File | Command | Container, compression type |
|---|---|---|
| `s8.aiff` | `afconvert -f AIFF -d I8 s8src.wav s8.aiff` | AIFF |
| `s16.aiff` | `afconvert -f AIFF -d BEI16 s16.wav s16.aiff` | AIFF |
| `s24.aiff` | `afconvert -f AIFF -d BEI24 s24.wav s24.aiff` | AIFF |
| `s32.aiff` | `afconvert -f AIFF -d BEI32 s32.wav s32.aiff` | AIFF |
| `s8-none.aifc` | `afconvert -f AIFC -d I8 s8src.wav s8-none.aifc` | AIFC `NONE` |
| `u8-raw.aifc` | `afconvert -f AIFC -d UI8 u8.wav u8-raw.aifc` | AIFC `raw ` |
| `s16-twos.aifc` | `afconvert -f AIFC -d BEI16 s16.wav s16-twos.aifc` | AIFC `twos` |
| `s24-in24.aifc` | `afconvert -f AIFC -d BEI24 s24.wav s24-in24.aifc` | AIFC `in24` |
| `s32-in32.aifc` | `afconvert -f AIFC -d BEI32 s32.wav s32-in32.aifc` | AIFC `in32` |
| `f32.aifc` | `afconvert -f AIFC -d BEF32 f32.wav f32.aifc` | AIFC `fl32` |
| `f64.aifc` | `afconvert -f AIFC -d BEF64 f64.wav f64.aifc` | AIFC `fl64` |

Types were read from a hex dump of each file's `COMM` chunk. `sowt` (little-endian) cannot be
produced by this `afconvert`; the reader tests build that case by hand.
