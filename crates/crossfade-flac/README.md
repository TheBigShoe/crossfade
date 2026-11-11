# crossfade-flac

FLAC (RFC 9639) in pure Rust: frame and STREAMINFO parsing, a verifying decoder, an
encoder, and the bit-level tools they share (bit reader and writer, CRC-8 and CRC-16, MD5).
No dependencies, no `unsafe`.

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

`Encoder` takes interleaved `i32` samples (sign-extended, any depth from 4 to 32 bits, 1 to 8
channels) and appends one frame per block of `block_size()` samples. Fixed block size; the
last block may be short.

```rust
use crossfade_flac::{Encoder, EncoderConfig};

let mut encoder = Encoder::new(EncoderConfig::new(44_100, 2, 16))?; // level 5
let mut frames = Vec::new();
for chunk in pcm_chunks {
    encoder.write(chunk, &mut frames)?; // any chunk size; frames appear as blocks fill
    // ... send `frames` on and clear it
}
encoder.flush(&mut frames)?; // the short final block
let info = encoder.finish(); // STREAMINFO: sizes, total samples, MD5
let streaminfo_body = info.write(); // the 34 bytes of the metadata block
```

`encode_block` is the exact-block form of the same thing. Levels 0 to 8 trade speed for size
as the reference encoder's presets do (the module documentation of `encode` has the table):
constant and verbatim subframes, wasted bits, fixed predictors, linear prediction from level
3, exact Rice partition search, and left/side, side/right and mid/side stereo.

- **Never worse than raw**: a frame is never larger than a verbatim one.
- **Exact decisions**: sizes are counted, not estimated, wherever two codings compete.
- **Deterministic**: no threads, and floating point only through operations IEEE 754 defines
  exactly, so the same input gives the same bytes on every platform.
- **Bounded**: one block of working memory, reused; nothing is allocated per block.

Measured against the reference `flac` 1.5.0 on the same PCM (development only; nothing in
the tests calls it), output sizes at the same level are within 0.3% on music (usually a
little smaller, up to 2% smaller on a 5.1 file), and every output passes `flac -t` and
decodes back to the source. On 60 s of 44.1 kHz 16-bit stereo this encoder takes about
68 / 125 / 273 ms at levels 0 / 5 / 8 on an Apple M-series laptop, against 120 / 204 / 300 ms
for the third-party encoder the engine used before. At levels 0 to 5 a good part of the time is
the MD5 of the audio. `examples/flac_encode.rs` writes a `.flac` from raw PCM or WAV for such
comparisons; `cargo bench -p crossfade-flac` has the speed baselines.

## Licence

MIT OR Apache-2.0.
