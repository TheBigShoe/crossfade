# crossfade-alac

Apple Lossless (ALAC) decoding and encoding in pure Rust. No dependencies, no `unsafe`, integer
arithmetic only, so it gives the same samples, and writes the same bytes, on every platform.

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

## Encoding

The encoder takes one packet's worth of interleaved samples at a time, in the same form the
decoder gives them back: right-justified at the bit depth, in ALAC's channel order. Every packet
holds the config's frame length (4096 unless set), only the last may be shorter. Once the last
packet is written, `config()` gives the cookie a container stores, with the largest packet and
the average bit rate filled in.

```rust
use crossfade_alac::{Encoder, EncoderConfig};

fn encode_all(pcm: &[i32]) -> crossfade_alac::Result<(Vec<u8>, Vec<Vec<u8>>)> {
    // 44.1 kHz stereo, 16-bit; `frame_length` can be changed before `Encoder::new`.
    let settings = EncoderConfig::new(44100, 2, 16);
    let mut encoder = Encoder::new(settings)?;
    let mut packets = Vec::new();
    for chunk in pcm.chunks(settings.frame_length as usize * 2) {
        let mut packet = Vec::new();
        encoder.encode(chunk, &mut packet)?;
        packets.push(packet);
    }
    // The 24-byte ALACSpecificConfig, as `Config::parse` reads it.
    let cookie = encoder.config().write().to_vec();
    Ok((cookie, packets))
}
```

What it writes is what Apple's encoder writes: the elements `afconvert` uses for each channel
count (single channel and channel pair elements; the LFE channel goes in a single channel
element), Apple's Rice tuning in the cookie, compressed frames (the adaptive FIR predictor at
orders 4 and 8 with its coefficients carried from packet to packet, Apple's generalised
mid-side mixes for a pair, low bytes of 24- and 32-bit samples passed through uncoded) and
uncompressed frames whenever compression would be no smaller, so no packet is larger than its
samples raw. Every choice is made by counting the bits each would take on the whole frame.
Unlike Apple's encoder it may also shift fewer bytes off 24- and 32-bit samples (none, so
silence and zero low bytes cost nothing). On the `afconvert` fixtures of `crossfade-format`
its packets come to 0.9996 of `afconvert`'s size.

Bad input is an error, never a panic: a config the decoder could not take, no samples, a count
that is not whole frames or exceeds the frame length, a sample outside the bit depth, a packet
after a short one.

## Sources

Written from Apple's format description (`ALACMagicCookieDescription.txt`) and the Apache-2.0
reference sources (github.com/macosforge/alac), read for the format's facts; no code was
copied. The encoder's search (the orders, mixes, shifts and starting coefficients it tries, the
passes that settle the coefficients) follows Apple's reference encoder, read the same way. The channel layouts are Apple's for each count (`ALACAudioTypes.h`), with Core Audio's
surround labels as WAVE's side pair (what `afconvert` does). Tests decode files `afconvert`
encoded and compare them with the source PCM sample for sample (`crossfade-format`'s
`tests/mp4_fixtures.rs`); this crate's own tests build bitstreams with a test-side coder, and
run everything the encoder writes back through the decoder.
