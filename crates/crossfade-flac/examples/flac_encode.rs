//! Development tool: PCM (raw or WAV) in, a native `.flac` out, with the encoder's own timing.
//!
//! ```text
//! flac_encode [--level N] [--rate HZ --channels N --bits N] [--time N] IN OUT
//! ```
//!
//! Raw input is signed little-endian, `ceil(bits / 8)` bytes per sample; a WAV (PCM, any
//! width) supplies its own format. `--time N` encodes N times and prints the best
//! encode-only time (input already in memory) to stderr.
use crossfade_flac::{Encoder, EncoderConfig};
use std::time::Instant;

fn wav(data: &[u8]) -> Option<(EncoderConfig, Vec<i32>)> {
    if data.get(..4)? != b"RIFF" || data.get(8..12)? != b"WAVE" {
        return None;
    }
    let (mut at, mut fmt, mut body) = (12, None, None);
    while at + 8 <= data.len() {
        let id = &data[at..at + 4];
        let len = u32::from_le_bytes(data[at + 4..at + 8].try_into().ok()?) as usize;
        let end = (at + 8 + len).min(data.len());
        match id {
            b"fmt " => fmt = Some(&data[at + 8..end]),
            b"data" => body = Some(&data[at + 8..end]),
            _ => {}
        }
        at = end + (len & 1);
    }
    let fmt = fmt?;
    let channels = u16::from_le_bytes(fmt[2..4].try_into().ok()?) as u8;
    let rate = u32::from_le_bytes(fmt[4..8].try_into().ok()?);
    let bits = u16::from_le_bytes(fmt[14..16].try_into().ok()?) as u8;
    let samples = decode(body?, bits, bits == 8);
    Some((EncoderConfig::new(rate, channels, bits), samples))
}

fn decode(bytes: &[u8], bits: u8, unsigned8: bool) -> Vec<i32> {
    let width = usize::from(bits).div_ceil(8);
    bytes
        .chunks_exact(width)
        .map(|c| {
            let mut w = [0u8; 4];
            w[..width].copy_from_slice(c);
            let v = i32::from_le_bytes(w) << (32 - 8 * width) >> (32 - 8 * width);
            if unsigned8 { v - 128 } else { v }
        })
        .collect()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut level, mut rate, mut channels, mut bits, mut times) =
        (5u8, 44_100u32, 2u8, 16u8, 0usize);
    let mut files = Vec::new();
    while let Some(a) = args.next() {
        let mut num = |name: &str| -> u32 {
            args.next()
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| panic!("{name} needs a number"))
        };
        match a.as_str() {
            "--level" => level = num("--level") as u8,
            "--rate" => rate = num("--rate"),
            "--channels" => channels = num("--channels") as u8,
            "--bits" => bits = num("--bits") as u8,
            "--time" => times = num("--time") as usize,
            _ => files.push(a),
        }
    }
    let [input, output] = files.as_slice() else {
        panic!("usage: flac_encode [options] IN OUT");
    };
    let data = std::fs::read(input).expect("read input");
    let (mut config, samples) = wav(&data).unwrap_or_else(|| {
        (
            EncoderConfig::new(rate, channels, bits),
            decode(&data, bits, false),
        )
    });
    config.level = level;

    let encode = || {
        let mut enc = Encoder::new(config).expect("config");
        let per = enc.block_size() * usize::from(config.channels);
        let mut frames = Vec::with_capacity(samples.len() * 2);
        for chunk in samples.chunks(per) {
            enc.encode_block(chunk, &mut frames).expect("encode");
        }
        (enc.finish(), frames)
    };
    let mut best = f64::MAX;
    for _ in 0..times.max(1) {
        let t = Instant::now();
        let out = encode();
        best = best.min(t.elapsed().as_secs_f64());
        std::hint::black_box(&out);
    }
    let (info, frames) = encode();
    let mut file = b"fLaC".to_vec();
    file.extend([0x80, 0, 0, 34]);
    file.extend(info.write());
    file.extend(frames);
    std::fs::write(output, &file).expect("write output");
    if times > 0 {
        eprintln!("encode {best:.4} s");
    }
    println!("{} bytes", file.len());
}
