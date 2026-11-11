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

`Decoder::with_threads(&info, threads)` adds a push interface that decodes on worker threads
(1, what `new` gives, starts none; `Decoder::AUTO_THREADS` picks up to 3, since a frame decodes
several times faster than the MD5 hashes it):

```rust
let mut decoder = Decoder::with_threads(&info, Decoder::AUTO_THREADS)?;
for frame in frames {
    decoder.push_frame(frame, &mut |block| { /* blocks arrive in order, a few frames late */ })?;
}
decoder.flush(&mut |block| { /* the rest */ })?;
assert_eq!(decoder.finish()?, Verification::Md5Matched);
```

Whatever the thread count, the blocks, the verification and the errors are the same: when a
frame fails, every block before it is delivered, then the error `decode_frame` gives for it,
and nothing after. A frame's header is checked by the call that pushes it. The MD5 runs on a
thread of its own, which with threads is the floor: about 650 MB/s of PCM.

Parameters that change mid-stream are rejected as `Error::Unsupported`. Arithmetic on
hostile data wraps instead of panicking; the frame CRC reports the damage.

Decoding always verifies: every frame's CRC-16, and the MD5 of the whole stream. With the
file in memory, splitting it into frames (`crossfade-format`'s reader) and decoding and
verifying every frame takes about 32 / 40 / 45 ms for 60 s of 44.1 kHz 16-bit stereo coded by
the reference `flac` 1.5.0 at levels 0 / 5 / 8, on an Apple M1 Max, where `flac -t` takes
48 / 64 / 65 ms of wall time; on 10 s of 96 kHz 24-bit stereo, 16 / 18 / 21 ms against
25 / 27 / 28 ms. The MD5 is about a third of that time on 16-bit audio and half on 24-bit,
and it is one serial chain. On two threads (`push_frame`) the same 60 s decode and verify in
about 16 ms and the 10 s of 24-bit audio in 9 ms, the MD5's own pace.
`crossfade-format`'s `examples/profile_flac_read.rs` measures it (`--threads N`).

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

`EncoderConfig::threads` lets `write` code frames on worker threads (1, what `new` sets,
keeps everything on the calling thread and starts none; `EncoderConfig::AUTO_THREADS` picks
from the machine). Frames then come out a few blocks later, in order, and `flush` appends the
rest; the MD5 runs on a thread of its own. The bytes are the same at every thread count.

`encode_block` is the exact-block form of the same thing, always on the calling thread. Levels 0 to 8 trade speed for size
as the reference encoder's presets do (the module documentation of `encode` has the table):
constant and verbatim subframes, wasted bits, fixed predictors, linear prediction from level
3, exact Rice partition search, and left/side, side/right and mid/side stereo.

- **Never worse than raw**: a frame is never larger than a verbatim one.
- **Exact decisions where they count**: the codings that compete for a subframe are compared
  by their exact sizes; where a level tries more linear predictors than it codes, the ones it
  codes are chosen by the error energy the block's covariance predicts for each.
- **Deterministic**: a frame depends only on its block, its number and the configuration, and
  floating point goes only through operations IEEE 754 defines exactly, so the same input
  gives the same bytes on every platform and at every thread count.
- **Bounded**: one block of working memory per thread, reused; at most `2 * threads` blocks in
  flight; nothing is allocated per block in steady state.

Measured against the reference `flac` 1.5.0 on the same PCM (development only; nothing in
the tests calls it), output sizes at the same level are within 0.3% on music (usually a
little smaller, up to 2% smaller on a 5.1 file), and every output passes `flac -t` and
decodes back to the source. On 60 s of 44.1 kHz 16-bit stereo this encoder takes about
40 / 70 / 131 ms at levels 0 / 5 / 8 in-process on an Apple M1 Max, where the reference `flac`
takes 61 / 98 / 206 ms for the same audio (wall time, file in and out); on 10 s of 96 kHz
24-bit stereo, 21 / 37 / 63 ms against 32 / 45 / 91 ms. At levels 0 to 5 about a fifth of the
time is the MD5 of the audio, which no encoder can skip. With threads (7, the automatic count
on 8 performance and 2 efficiency cores) the same 60 s take about 18 / 18 / 23 ms in-process,
and the 10 s of 24-bit audio 10 / 9 / 9 ms; the MD5, one serial chain at about 650 MB/s, then
sets the pace at levels 0 to 5. `examples/flac_encode.rs` writes a
`.flac` from raw PCM or WAV for such comparisons (`--time N` gives the best of N runs);
`cargo bench -p crossfade-flac` has the speed baselines.

## Licence

MIT OR Apache-2.0.
