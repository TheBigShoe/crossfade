# crossfade-flac

FLAC (RFC 9639) in pure Rust: frame and STREAMINFO parsing, a verifying decoder, and the
bit-level tools they share (bit reader and writer, CRC-8 and CRC-16, MD5). No dependencies,
no `unsafe`.

## Decoding

A container reader (FLAC file, Ogg) cuts the stream into frames; the decoder takes one whole
frame at a time and checks its CRC-16.

```rust
use crossfade_flac::{Decoder, Verification, streaminfo::StreamInfoBlock};

let info = StreamInfoBlock::parse(&streaminfo_body)?; // the 34-byte block body
let mut decoder = Decoder::new(&info)?;
let mut pcm = Vec::new();
for frame in frames {
    let block = decoder.decode_frame(frame)?;
    block.interleave_i32(32 - u32::from(block.bits_per_sample()), &mut pcm);
    // ... use pcm, or read block.channel(i) for raw per-channel values
}
// The sample total and the MD5 from STREAMINFO, when it has them.
assert_eq!(decoder.finish()?, Verification::Md5Matched);
```

Parameters that change mid-stream are rejected as `Error::Unsupported`. Arithmetic on
hostile data wraps instead of panicking; the frame CRC reports the damage.

## Encoding

An encoder is coming to this crate.

## Licence

MIT OR Apache-2.0.
