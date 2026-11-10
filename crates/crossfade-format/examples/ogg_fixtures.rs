//! Writes the raw PCM the Ogg FLAC fixtures are encoded from; tests/fixtures/ogg/README.md
//! has the `flac` commands. Output is little-endian, signed, interleaved.
//!
//! `cargo run -p crossfade-format --example ogg_fixtures -- <dir>`

use std::f64::consts::TAU;
use std::path::PathBuf;

fn next(state: &mut u64) -> u64 {
    // xorshift64*: fixed, tiny and platform independent.
    *state ^= *state >> 12;
    *state ^= *state << 25;
    *state ^= *state >> 27;
    state.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

/// A two-tone signal with a little noise, so the encoder has something to predict.
fn tone(frame: usize, rate: f64, channel: usize, noise: &mut u64) -> f64 {
    let t = frame as f64 / rate;
    let f = 220.0 * (channel as f64 + 1.0);
    let n = (next(noise) >> 40) as f64 / (1u64 << 24) as f64 - 0.5;
    0.5 * (TAU * f * t).sin() + 0.2 * (TAU * 3.0 * f * t).sin() + 0.02 * n
}

fn pcm(frames: usize, channels: usize, bytes: usize, rate: f64) -> Vec<u8> {
    let mut noise = 1;
    let full = (1i64 << (bytes * 8 - 1)) as f64 - 1.0;
    let mut out = Vec::new();
    for i in 0..frames {
        for c in 0..channels {
            let v = (tone(i, rate, c, &mut noise) * full) as i64;
            out.extend_from_slice(&v.to_le_bytes()[..bytes]);
        }
    }
    out
}

fn main() -> std::io::Result<()> {
    let dir = PathBuf::from(std::env::args().nth(1).expect("usage: ogg_fixtures <dir>"));
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("stereo16.raw"), pcm(4410, 2, 2, 44100.0))?;
    std::fs::write(dir.join("mono24.raw"), pcm(4800, 1, 3, 48000.0))?;
    let mut state = 7;
    let noise: Vec<u8> = (0..16384).map(|_| (next(&mut state) >> 56) as u8).collect();
    std::fs::write(dir.join("noise8.raw"), noise)
}
