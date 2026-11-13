# crossfade-alac

Apple Lossless (ALAC) decoding in pure Rust. No dependencies, no `unsafe`, integer arithmetic
only, so it gives the same samples on every platform.

## Decoding

A container reader (MP4, CAF) gives the stream's ALACSpecificConfig, the 24-byte "magic
cookie", and cuts the stream into packets; the decoder takes one packet at a time. Packets are
independent: any one can be decoded at any time, so a seek needs no reset.

```rust
use crossfade_alac::{Config, Decoder, wave_layout};

fn decode_all(cookie: &[u8], packets: &[&[u8]]) -> crossfade_alac::Result<Vec<i32>> {
    // 24 bytes, or wrapped in `frma`/`alac` atoms as QuickTime and MP4 store them.
    let config = Config::parse(cookie)?;
    let mut decoder = Decoder::new(config)?;
    let (mut all, mut pcm) = (Vec::new(), Vec::new());
    for packet in packets {
        // `pcm`: frames * channels interleaved samples, sign-extended, right-justified at
        // config.bit_depth, in ALAC's channel order.
        let _frames = decoder.decode(packet, &mut pcm)?;
        all.extend_from_slice(&pcm);
    }
    // ALAC's order (5.1: C L R Ls Rs LFE) as WAVE speaker positions, and for each WAVE slot
    // the ALAC channel that fills it.
    let _layout = wave_layout(config.channels);
    Ok(all)
}
```

What it decodes: 16-, 20-, 24- and 32-bit audio, 1 to 8 channels; single channel, channel
pair and LFE elements, data stream and fill elements (skipped); compressed frames (the
adaptive Golomb-Rice coder with its zero runs, the adaptive FIR predictor in its plain and its
"prediction on prediction" mode, mid-side de-matrixing, shifted low bytes above 16 bits) and
uncompressed (escape) frames; a shorter last frame. Coupling channel and program config
elements, which no ALAC encoder writes and which carry no length to skip by, are refused as
`Error::Unsupported`.

Hostile packets never panic: arithmetic wraps as the reference decoder's does, every read is
bounds-checked, and the decoder allocates its buffers (a few times `frame_length` samples, plus
the caller's output of `frame_length * channels`) once, in `Decoder::new`. `Config::check`
caps `frame_length` at 2^20.

## Sources

Written from Apple's format description (`ALACMagicCookieDescription.txt`) and the Apache-2.0
reference sources (github.com/macosforge/alac), read for the format's facts; no code was
copied. The channel layouts are Apple's for each count (`ALACAudioTypes.h`), with Core Audio's
surround labels as WAVE's side pair (what `afconvert` does). Tests decode files `afconvert`
encoded and compare them with the source PCM sample for sample (`crossfade-format`'s
`tests/mp4_fixtures.rs`); this crate's own tests build bitstreams with a test-side coder.
