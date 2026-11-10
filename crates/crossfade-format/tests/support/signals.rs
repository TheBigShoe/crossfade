//! The deterministic signals the FLAC fixtures were encoded from. Integer arithmetic only, so
//! every platform regenerates the same samples; the fixtures' README lists the encoder runs.
// Included by tests and an example, each of which uses a different part.
#![allow(dead_code, unreachable_pub)]

pub struct Signal {
    pub rate: u32,
    pub channels: usize,
    pub bits: u8,
    /// Interleaved raw sample values, each within the signed range of `bits`.
    pub samples: Vec<i32>,
}

impl Signal {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels
    }

    /// Little-endian signed PCM in `ceil(bits / 8)` bytes per sample: the encoder's raw input.
    pub fn raw_pcm(&self) -> Vec<u8> {
        let width = usize::from(self.bits).div_ceil(8);
        let mut out = Vec::with_capacity(self.samples.len() * width);
        for s in &self.samples {
            out.extend_from_slice(&s.to_le_bytes()[..width]);
        }
        out
    }
}

impl Signal {
    /// WAVE_FORMAT_EXTENSIBLE with the samples left-justified in the container and the real
    /// depth in `wValidBitsPerSample`: the way to hand the encoder a 12 or 20 bit signal.
    pub fn wav_extensible(&self) -> Vec<u8> {
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
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&[
            1, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71,
        ]);
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);
        out
    }
}

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// Uniform in `-amp..=amp`.
    pub fn noise(&mut self, amp: i64) -> i64 {
        (self.next() >> 11) as i64 % (2 * amp + 1) - amp
    }
}

/// A smooth wave of amplitude about 32767 from a 32-bit phase: a parabola over a triangle,
/// which needs no libm and so is identical everywhere.
fn tone(phase: u32) -> i64 {
    let saw = i64::from(phase >> 16) - 0x8000;
    let tri = 2 * saw.abs() - 0x8000;
    (tri * (0x1_0000 - tri.abs()) / 0x8000).clamp(-32767, 32767)
}

fn scale(v: i64, bits: u8) -> i64 {
    if bits >= 16 {
        v << (bits - 16)
    } else {
        v >> (16 - bits)
    }
}

fn clamp(v: i64, bits: u8) -> i32 {
    let max = (1i64 << (bits - 1)) - 1;
    v.clamp(-max - 1, max) as i32
}

/// Two tones per channel plus a little noise; the channels share a tone, so they correlate.
fn tonal(rate: u32, channels: usize, bits: u8, frames: usize, seed: u64) -> Signal {
    let mut rng = Rng::new(seed);
    let mut samples = Vec::with_capacity(frames * channels);
    let mut phases = vec![(0u32, 0u32); channels];
    for _ in 0..frames {
        for (c, (a, b)) in phases.iter_mut().enumerate() {
            *a = a.wrapping_add(0x0300_0000 + c as u32 * 0x0040_0000);
            *b = b.wrapping_add(0x0B00_0000 + c as u32 * 0x0100_0000);
            let v = tone(*a) / 2 + tone(*b) / 4 + rng.noise(24);
            samples.push(clamp(scale(v, bits), bits));
        }
    }
    Signal {
        rate,
        channels,
        bits,
        samples,
    }
}

fn extreme_32() -> Signal {
    // Opposite full-scale values make the side channel need 33 bits.
    let mut samples = Vec::new();
    let mut rng = Rng::new(0x32);
    let mut phase = 0u32;
    for n in 0..3000 {
        phase = phase.wrapping_add(0x0200_0000);
        if (400..600).contains(&n) {
            let hi = n % 2 == 0;
            samples.push(if hi { i32::MAX } else { i32::MIN });
            samples.push(if hi { i32::MIN } else { i32::MAX });
        } else if (1200..1300).contains(&n) {
            samples.push(clamp(i64::from(i32::MAX) - rng.noise(3), 32));
            samples.push(clamp(i64::from(i32::MIN) + rng.noise(3), 32));
        } else {
            let l = scale(tone(phase), 32) + rng.noise(1 << 12);
            samples.push(clamp(l, 32));
            samples.push(clamp(-l / 2 + rng.noise(1 << 12), 32));
        }
    }
    Signal {
        rate: 48000,
        channels: 2,
        bits: 32,
        samples,
    }
}

/// Four stretches of noise in which one stereo decorrelation mode is cheapest: left/side,
/// right/side, mid/side and independent channels.
fn stereo_modes() -> Signal {
    let mut rng = Rng::new(0x57E);
    let mut samples = Vec::new();
    for kind in 0..4 {
        for _ in 0..2048 {
            let w = rng.noise(256);
            let (l, r) = match kind {
                0 => (w, w + rng.noise(128)),
                1 => (w + rng.noise(128), w),
                2 => {
                    let q = rng.noise(80);
                    (w + q, w - q)
                }
                _ => (w, rng.noise(256)),
            };
            samples.push(clamp(l, 16));
            samples.push(clamp(r, 16));
        }
    }
    Signal {
        rate: 44100,
        channels: 2,
        bits: 16,
        samples,
    }
}

/// Blocks of 192 mono 24-bit samples (the encoder is run with `-b 192`), five kinds twice over:
/// each is a signal one fixed predictor order fits best, from white noise (order 0) to an exact
/// cubic (order 4). Two more blocks have a quiet first half and a full-range noise second half,
/// so their two partitions need very different Rice parameters.
fn fixed_orders() -> Signal {
    let mut rng = Rng::new(0xF1ED);
    let mut samples = Vec::new();
    for _ in 0..2 {
        for _ in 0..192 {
            samples.push(rng.noise(100_000) as i32);
        }
        for _ in 0..192 {
            samples.push((4_000_000 + rng.noise(300)) as i32);
        }
        for n in 0..192i64 {
            samples.push((40_000 * n - 3_800_000 + rng.noise(40)) as i32);
        }
        for n in 0..192i64 {
            samples.push((200 * n * n - 3_700_000) as i32);
        }
        for n in 0..192i64 {
            samples.push((n * n * n + 5 * n * n - 100 * n - 3_500_000) as i32);
        }
    }
    for _ in 0..2 {
        for i in 0..192 {
            samples.push(if i < 96 {
                rng.noise(2)
            } else {
                rng.noise(8_000_000)
            } as i32);
        }
    }
    Signal {
        rate: 48000,
        channels: 1,
        bits: 24,
        samples,
    }
}

pub fn signal(name: &str) -> Signal {
    match name {
        "fixed-orders" => fixed_orders(),
        "mono8" => tonal(8000, 1, 8, 6000, 8),
        "stereo12" => tonal(44100, 2, 12, 5000, 12),
        "stereo16" => tonal(44100, 2, 16, 5000, 16),
        "stereo20" => tonal(48000, 2, 20, 4000, 20),
        "stereo24" => tonal(96000, 2, 24, 3000, 24),
        "stereo32" => extreme_32(),
        "surround6" => tonal(48000, 6, 16, 2000, 6),
        "noise16" => {
            let mut rng = Rng::new(0xA015E);
            Signal {
                rate: 44100,
                channels: 2,
                bits: 16,
                samples: (0..9000).map(|_| rng.noise(32767) as i32).collect(),
            }
        }
        "silence" => Signal {
            rate: 44100,
            channels: 1,
            bits: 16,
            samples: vec![0; 12000],
        },
        "wasted" => {
            let mut s = tonal(44100, 2, 16, 6000, 3);
            for (i, v) in s.samples.iter_mut().enumerate() {
                // Left keeps 13 significant bits, right 11.
                *v &= if i % 2 == 0 { !0x7 } else { !0x1F };
            }
            s
        }
        "stereo-modes" => stereo_modes(),
        other => panic!("no signal named {other}"),
    }
}

/// (file, signal) of every committed fixture.
pub const FIXTURES: &[(&str, &str)] = &[
    ("mono-8bit.flac", "mono8"),
    ("stereo-12bit.flac", "stereo12"),
    ("stereo-16bit.flac", "stereo16"),
    ("stereo-20bit.flac", "stereo20"),
    ("stereo-24bit.flac", "stereo24"),
    ("stereo-32bit.flac", "stereo32"),
    ("surround-5.1.flac", "surround6"),
    ("fixed-only.flac", "fixed-orders"),
    ("lpc-order-32.flac", "stereo16"),
    ("verbatim-noise.flac", "noise16"),
    ("constant-silence.flac", "silence"),
    ("wasted-bits.flac", "wasted"),
    ("stereo-modes-m.flac", "stereo-modes"),
    ("stereo-modes-adaptive.flac", "stereo-modes"),
    ("stereo-modes-none.flac", "stereo-modes"),
    ("block-16.flac", "stereo16"),
    ("block-4608.flac", "stereo16"),
    ("block-65535.flac", "stereo16"),
    ("block-1000.flac", "stereo16"),
    ("tagged.flac", "stereo16"),
    ("id3v2-prefixed.flac", "stereo16"),
    ("unknown-total.flac", "stereo16"),
];
