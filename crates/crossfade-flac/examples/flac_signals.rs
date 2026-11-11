//! Development tool: writes the synthetic test signals as raw signed little-endian PCM.
//!
//! ```text
//! flac_signals OUT_DIR
//! ```
//!
//! Each file is `name.RATE.CHANNELS.BITS.raw`.
use std::fs;
use std::path::Path;

struct Lcg(u32);

impl Lcg {
    fn noise(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        f64::from(self.0 >> 8) / f64::from(1u32 << 24) - 0.5
    }
}

/// Sine mix plus seeded noise, as the engine's FLAC benches build their signals.
fn tonal(rate: u32, channels: usize, bits: u32, seconds: u32) -> Vec<i32> {
    let amp = f64::from((1u32 << (bits - 1).min(30)) - 1) * if bits > 31 { 2.0 } else { 1.0 };
    let mut rng = Lcg(0x9e37_79b9);
    let mut out = Vec::new();
    for t in 0..rate as usize * seconds as usize {
        for ch in 0..channels {
            let x = t as f64;
            let tone = (x * (0.06 + 0.013 * ch as f64)).sin() * 0.4
                + (x * 0.0007).sin() * 0.25
                + (x * 0.0213).sin() * 0.1;
            out.push(((tone + rng.noise() * 0.02) * amp) as i32);
        }
    }
    out
}

/// Sines below 4 kHz, a slow amplitude sweep and a little noise: strongly band-limited when
/// the rate is high, so linear prediction can do much better than the noise floor suggests.
fn band(rate: u32, bits: u32, seconds: u32, shape: u8) -> Vec<i32> {
    let amp = f64::from((1u32 << (bits - 1)) - 1);
    let mut rng = Lcg(7);
    let mut out = Vec::new();
    let r = f64::from(rate);
    for t in 0..rate as usize * seconds as usize {
        let x = t as f64 / r;
        for ch in 0..2 {
            let c = f64::from(ch) * 0.3;
            let v = match shape {
                0 => {
                    0.3 * (std::f64::consts::TAU * 440.0 * x + c).sin()
                        + 0.2 * (std::f64::consts::TAU * 1300.0 * x).sin()
                        + 0.1 * (std::f64::consts::TAU * 3100.0 * x + c).sin()
                }
                // A sweep from 100 Hz to 3.9 kHz.
                1 => {
                    let f = 100.0 + 3800.0 * x / f64::from(seconds);
                    0.5 * (std::f64::consts::TAU * (100.0 * x + 0.5 * (f - 100.0) * x) + c).sin()
                }
                _ => 0.99 * (std::f64::consts::TAU * 1000.0 * x).sin(),
            };
            let noise = if shape == 2 {
                0.0
            } else {
                rng.noise() * 4.0 / amp
            };
            out.push(((v + noise) * amp) as i32);
        }
    }
    out
}

fn white(bits: u32, count: usize) -> Vec<i32> {
    let mut s = 0x1234_5678_9abc_def1u64;
    (0..count)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s as i64 >> (64 - bits)) as i32
        })
        .collect()
}

fn write(dir: &Path, name: &str, rate: u32, channels: usize, bits: u32, samples: &[i32]) {
    let width = (bits as usize).div_ceil(8);
    let mut bytes = Vec::with_capacity(samples.len() * width);
    for s in samples {
        bytes.extend_from_slice(&s.to_le_bytes()[..width]);
    }
    fs::write(
        dir.join(format!("{name}.{rate}.{channels}.{bits}.raw")),
        bytes,
    )
    .unwrap();
}

fn main() {
    let dir = std::env::args()
        .nth(1)
        .expect("usage: flac_signals OUT_DIR");
    let dir = Path::new(&dir);
    fs::create_dir_all(dir).unwrap();
    write(dir, "bench16", 44_100, 2, 16, &tonal(44_100, 2, 16, 60));
    write(dir, "bench24", 96_000, 2, 24, &tonal(96_000, 2, 24, 10));
    for bits in [8, 16, 24, 32] {
        write(
            dir,
            &format!("noise{bits}"),
            44_100,
            2,
            bits,
            &white(bits, 44_100 * 2 * 5),
        );
    }
    for (rate, bits) in [
        (48_000, 16),
        (48_000, 24),
        (96_000, 16),
        (96_000, 24),
        (192_000, 16),
        (192_000, 24),
    ] {
        write(
            dir,
            &format!("bl{rate}_{bits}"),
            rate,
            2,
            bits,
            &band(rate, bits, 6, 0),
        );
    }
    write(dir, "sweep192", 192_000, 2, 24, &band(192_000, 24, 6, 1));
    write(dir, "tone192", 192_000, 2, 24, &band(192_000, 24, 6, 2));
    write(dir, "silence", 44_100, 2, 16, &vec![0; 44_100 * 2 * 10]);
    write(dir, "sig12", 44_100, 2, 12, &tonal(44_100, 2, 12, 10));
    write(dir, "sig20", 48_000, 2, 20, &tonal(48_000, 2, 20, 10));
    write(dir, "multi8", 192_000, 8, 24, &tonal(192_000, 8, 24, 3));
    // Full scale: a loud tone with the extremes mixed in.
    let mut full = tonal(44_100, 2, 32, 10);
    for (i, v) in full.iter_mut().enumerate() {
        if i % 5000 == 0 {
            *v = if i % 10_000 == 0 { i32::MIN } else { i32::MAX };
        }
    }
    write(dir, "full32", 44_100, 2, 32, &full);
}
