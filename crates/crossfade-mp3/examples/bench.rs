//! Decoding speed in process, one thread, for measuring the decoder alone:
//! `cargo run --release -p crossfade-mp3 --example bench -- in.mp3 [runs] [out.f32]`.
//! Frames are split once up front; each run decodes them all with a fresh `Decoder` and the
//! best run is reported, with a digest of the output so a change can be checked for bits.
//! With `out.f32`, the samples of the last run are written as raw little-endian `f32`.

use crossfade_mp3::{Decoder, FrameHeader};
use std::time::Instant;

fn frames(bytes: &[u8]) -> Vec<&[u8]> {
    let mut at = 0;
    // An ID3v2 tag: ten bytes, then a 28-bit syncsafe size.
    if bytes.starts_with(b"ID3") && bytes.len() >= 10 {
        let s = &bytes[6..10];
        at = 10
            + ((usize::from(s[0]) << 21)
                | (usize::from(s[1]) << 14)
                | (usize::from(s[2]) << 7)
                | usize::from(s[3]));
    }
    let mut out = Vec::new();
    while let Some(head) = bytes.get(at..at + 4) {
        let len = FrameHeader::parse(head.try_into().unwrap_or(&[0; 4]))
            .ok()
            .and_then(|h| h.frame_len());
        match len {
            Some(len) if at + len <= bytes.len() => {
                out.push(&bytes[at..at + len]);
                at += len;
            }
            _ => at += 1,
        }
    }
    out
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let Some(input) = args.get(1) else {
        return Err("usage: bench <in.mp3> [runs] [out.f32]".into());
    };
    let runs: usize = args.get(2).map_or(Ok(9), |r| r.parse())?;
    let bytes = std::fs::read(input)?;
    let frames = frames(&bytes);
    let mut pcm = Vec::new();
    let mut best = f64::INFINITY;
    let (mut samples, mut rate) = (0usize, 0u32);
    for _ in 0..runs {
        pcm.clear();
        let mut decoder = Decoder::new();
        samples = 0;
        let start = Instant::now();
        for frame in &frames {
            if let Ok(info) = decoder.decode_frame(frame, &mut pcm) {
                samples += info.samples;
                rate = info.header.sample_rate;
            }
        }
        best = best.min(start.elapsed().as_secs_f64());
    }
    // FNV-1a over the bits of every sample.
    let digest = pcm.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, s| {
        (h ^ u64::from(s.to_bits())).wrapping_mul(0x100_0000_01b3)
    });
    let seconds = samples as f64 / f64::from(rate.max(1));
    println!(
        "{input}: {} frames, {seconds:.1} s of audio, best of {runs}: {:.1} ms, {:.0}x real time, digest {digest:016x}",
        frames.len(),
        best * 1e3,
        seconds / best
    );
    if let Some(path) = args.get(3) {
        let raw: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
        std::fs::write(path, raw)?;
    }
    Ok(())
}
