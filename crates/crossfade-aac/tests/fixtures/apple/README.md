# Apple reference windows

Each `<name>.f32` holds windows of Apple's decode of `crossfade-format/tests/fixtures/adts/<name>.aac`,
made with `afconvert -f WAVE -d LEF32 <name>.aac <name>.wav` (macOS 27.2): blocks of a `u32`
start frame, a `u32` frame count, then that many interleaved `f32` frames, little-endian. Windows
start at frame 2048, the middle and 2304 before the end (and at a click onset for
`clicks_44k_stereo`), 256 frames each. Apple's decode of ADTS starts at the stream's first frame
(no delay to align: its length is exactly 1024 per packet), as ours does.

`digests.txt` freezes an FNV-1a 64 digest of our whole output per stream (the HE-AAC one is its
core); rewrite it with `CROSSFADE_BLESS=1` after a deliberate change.
