//! The deterministic signals the MP4 fixtures were encoded from. Integer arithmetic only, so
//! every platform regenerates the same samples; `tests/fixtures/mp4/README.md` lists the
//! encoder runs.
// Included by tests and an example, each of which uses a different part.
#![allow(dead_code, unreachable_pub)]

pub struct Signal {
    pub rate: u32,
    pub channels: usize,
    pub bits: u8,
    /// Interleaved raw sample values, each within the signed range of `bits`, channels in WAVE
    /// order.
    pub samples: Vec<i32>,
}

impl Signal {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels
    }

    /// WAVE_FORMAT_EXTENSIBLE, samples left-justified in whole bytes, the real depth in
    /// `wValidBitsPerSample` and the default WAVE mask for the channel count.
    pub fn wav(&self) -> Vec<u8> {
        let width = usize::from(self.bits).div_ceil(8);
        let shift = width as u32 * 8 - u32::from(self.bits);
        let mut data = Vec::new();
        for s in &self.samples {
            data.extend_from_slice(&(s << shift).to_le_bytes()[..width]);
        }
        let ch = self.channels as u16;
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(4 + 8 + 40 + 8 + data.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&0xFFFEu16.to_le_bytes());
        out.extend_from_slice(&ch.to_le_bytes());
        out.extend_from_slice(&self.rate.to_le_bytes());
        out.extend_from_slice(&(self.rate * u32::from(ch) * width as u32).to_le_bytes());
        out.extend_from_slice(&(ch * width as u16).to_le_bytes());
        out.extend_from_slice(&(width as u16 * 8).to_le_bytes());
        out.extend_from_slice(&22u16.to_le_bytes());
        out.extend_from_slice(&u16::from(self.bits).to_le_bytes());
        out.extend_from_slice(&mask(self.channels).to_le_bytes());
        out.extend_from_slice(&[
            1, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71,
        ]);
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);
        out
    }
}

/// The WAVE masks of the layouts ALAC has for each channel count (see `crossfade_alac::layout`).
pub fn mask(channels: usize) -> u32 {
    match channels {
        1 => 0x4,
        2 => 0x3,
        3 => 0x7,
        4 => 0x107,
        5 => 0x607,
        6 => 0x60F,
        7 => 0x70F,
        8 => 0x6CF,
        _ => 0,
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// Uniform in `-amp..=amp`.
    fn noise(&mut self, amp: i64) -> i64 {
        (self.next() >> 11) as i64 % (2 * amp + 1) - amp
    }
}

/// A smooth wave of amplitude about 32767 from a 32-bit phase: a parabola over a triangle.
fn tone(phase: u32) -> i64 {
    let saw = i64::from(phase >> 16) - 0x8000;
    let tri = 2 * saw.abs() - 0x8000;
    (tri * (0x1_0000 - tri.abs()) / 0x8000).clamp(-32767, 32767)
}

fn clamp(v: i64, bits: u8) -> i32 {
    let max = (1i64 << (bits - 1)) - 1;
    v.clamp(-max - 1, max) as i32
}

/// Two tones per channel plus a little noise, scaled to `bits`; the channels share a tone, so
/// they correlate (stereo pairs get mid-side), and each has its own second tone.
fn tonal(rate: u32, channels: usize, bits: u8, frames: usize, seed: u64) -> Signal {
    let mut rng = Rng(seed);
    let mut samples = Vec::with_capacity(frames * channels);
    let mut phases = vec![(0u32, 0u32); channels];
    for _ in 0..frames {
        for (c, (a, b)) in phases.iter_mut().enumerate() {
            *a = a.wrapping_add(0x0300_0000);
            *b = b.wrapping_add(0x0B00_0000 + c as u32 * 0x0100_0000);
            let v = (tone(*a) / 2 + tone(*b) / 4 + rng.noise(24)) << (bits - 16);
            samples.push(clamp(v + rng.noise(1 << (bits - 16)), bits));
        }
    }
    Signal {
        rate,
        channels,
        bits,
        samples,
    }
}

pub fn signal(name: &str) -> Signal {
    match name {
        "mono16" => tonal(22050, 1, 16, 5001, 1),
        "stereo16" => tonal(44100, 2, 16, 10007, 2),
        "stereo20" => tonal(48000, 2, 20, 4001, 20),
        "stereo24" => tonal(96000, 2, 24, 4501, 24),
        "stereo32" => tonal(48000, 2, 32, 3001, 32),
        "ch3" => tonal(48000, 3, 16, 2001, 3),
        "ch4" => tonal(48000, 4, 16, 2001, 4),
        "ch5" => tonal(48000, 5, 16, 2001, 5),
        "ch6" => tonal(48000, 6, 16, 2001, 6),
        "ch7" => tonal(48000, 7, 16, 2001, 7),
        "ch8" => tonal(48000, 8, 24, 2001, 8),
        "noise16" => {
            let mut rng = Rng(0xA1AC);
            Signal {
                rate: 44100,
                channels: 2,
                bits: 16,
                samples: (0..2 * 5000).map(|_| rng.noise(32767) as i32).collect(),
            }
        }
        "silence" => Signal {
            rate: 44100,
            channels: 1,
            bits: 16,
            samples: vec![0; 9000],
        },
        other => panic!("no signal named {other}"),
    }
}

/// Every signal a fixture was made from.
pub const SIGNALS: &[&str] = &[
    "mono16", "stereo16", "stereo20", "stereo24", "stereo32", "ch3", "ch4", "ch5", "ch6", "ch7",
    "ch8", "noise16", "silence",
];
