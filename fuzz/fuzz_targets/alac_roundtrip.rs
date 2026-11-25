#![no_main]
//! The ALAC encode/decode property: whatever samples come in, at any depth, channel count and
//! frame length, the decoder gives back exactly what the encoder was handed, and no packet is
//! larger than its uncompressed form. Panics by design when that fails.

use crossfade_alac::{Config, Decoder, Encoder, EncoderConfig};
use libfuzzer_sys::fuzz_target;

/// Keeps one run fast while still spanning several packets.
const MAX_FRAMES: usize = 20_000;

fuzz_target!(|data: &[u8]| {
    let [depth, channels, len_hi, len_lo, body @ ..] = data else {
        return;
    };
    let depth = [16u8, 20, 24, 32][usize::from(*depth) % 4];
    let channels = *channels % 8 + 1;
    let frame_length = u32::from(u16::from_be_bytes([*len_hi, *len_lo]) % 4096) + 1;
    let mut settings = EncoderConfig::new(44100, channels, depth);
    settings.frame_length = frame_length;
    let mut encoder = Encoder::new(settings).unwrap();

    // Four bytes per sample, sign-extended from the stream's depth so every value is in range.
    let shift = 32 - u32::from(depth);
    let per_frame = 4 * usize::from(channels);
    let frames = (body.len() / per_frame).min(MAX_FRAMES);
    let pcm: Vec<i32> = body[..frames * per_frame]
        .chunks_exact(4)
        .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]) << shift >> shift)
        .collect();

    let per_packet = frame_length as usize * usize::from(channels);
    let mut packets = Vec::new();
    for chunk in pcm.chunks(per_packet) {
        let mut out = Vec::new();
        encoder.encode(chunk, &mut out).unwrap();
        // Per element at most a tag, a header and a length beside the samples.
        let raw = chunk.len() * usize::from(depth) + 8 * (3 + 4 + 16 + 32) + 3;
        assert!(out.len() <= raw.div_ceil(8), "a packet larger than raw");
        packets.push(out);
    }

    let config = Config::parse(&encoder.config().write()).expect("our own cookie parses");
    let mut decoder = Decoder::new(config).unwrap();
    let (mut back, mut buf) = (Vec::new(), Vec::new());
    for p in &packets {
        decoder.decode(p, &mut buf).expect("our own packet decodes");
        back.extend_from_slice(&buf);
    }
    assert!(back == pcm, "decoded samples differ from the input");
});
