//! The STREAMINFO metadata block body (RFC 9639 section 8.2).

use crate::bits::{BitReader, BitWriter};
use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamInfoBlock {
    pub min_block_size: u16,
    pub max_block_size: u16,
    /// 24-bit; 0 = unknown.
    pub min_frame_size: u32,
    pub max_frame_size: u32,
    /// 20-bit, 1 to 1_048_575.
    pub sample_rate: u32,
    /// 1 to 8.
    pub channels: u8,
    /// 4 to 32.
    pub bits_per_sample: u8,
    /// 36-bit; 0 = unknown.
    pub total_samples: u64,
    /// All zero = unknown.
    pub md5: [u8; 16],
}

fn malformed(what: &str) -> Error {
    Error::Malformed(format!("flac streaminfo: {what}"))
}

impl StreamInfoBlock {
    pub const LEN: usize = 34;

    pub fn parse(body: &[u8]) -> Result<Self> {
        if body.len() != Self::LEN {
            return Err(malformed("body is not 34 bytes"));
        }
        let mut r = BitReader::new(body);
        // The length check above makes every read below succeed.
        let mut bits = |n| r.read_bits(n);
        let min_block_size = bits(16)? as u16;
        let max_block_size = bits(16)? as u16;
        let min_frame_size = bits(24)? as u32;
        let max_frame_size = bits(24)? as u32;
        let sample_rate = bits(20)? as u32;
        let channels = bits(3)? as u8 + 1;
        let bits_per_sample = bits(5)? as u8 + 1;
        let total_samples = bits(36)?;
        let mut md5 = [0u8; 16];
        for b in &mut md5 {
            *b = bits(8)? as u8;
        }
        if sample_rate == 0 {
            return Err(malformed("sample rate is zero"));
        }
        if min_block_size < 16 || max_block_size < 16 {
            return Err(malformed("block size below 16"));
        }
        if min_block_size > max_block_size {
            return Err(malformed("minimum block size exceeds maximum"));
        }
        if bits_per_sample < 4 {
            return Err(malformed("bits per sample below 4"));
        }
        Ok(Self {
            min_block_size,
            max_block_size,
            min_frame_size,
            max_frame_size,
            sample_rate,
            channels,
            bits_per_sample,
            total_samples,
            md5,
        })
    }

    /// Fields wider than their slot are masked in release builds.
    pub fn write(&self) -> [u8; 34] {
        debug_assert!(self.min_frame_size < 1 << 24 && self.max_frame_size < 1 << 24);
        debug_assert!(self.sample_rate < 1 << 20);
        debug_assert!((1..=8).contains(&self.channels));
        debug_assert!((1..=32).contains(&self.bits_per_sample));
        debug_assert!(self.total_samples < 1 << 36);
        let mut w = BitWriter::new();
        w.write_bits(u64::from(self.min_block_size), 16);
        w.write_bits(u64::from(self.max_block_size), 16);
        w.write_bits(u64::from(self.min_frame_size), 24);
        w.write_bits(u64::from(self.max_frame_size), 24);
        w.write_bits(u64::from(self.sample_rate), 20);
        w.write_bits(u64::from(self.channels.wrapping_sub(1)), 3);
        w.write_bits(u64::from(self.bits_per_sample.wrapping_sub(1)), 5);
        w.write_bits(self.total_samples, 36);
        for b in self.md5 {
            w.write_bits(u64::from(b), 8);
        }
        let mut out = [0u8; 34];
        out.copy_from_slice(&w.into_bytes());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn hx(s: &str) -> Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
            .collect()
    }

    // `flac -s --no-padding -b 4096 -o a.flac a.wav` (10000 frames of 44.1 kHz stereo 16-bit
    // sine), body = bytes 8..42 of the file (`xxd -s 8 -l 34 -p`). Fields cross-checked with
    // `metaflac --list --block-type=STREAMINFO a.flac`.
    const A: &str = "100010000005a3000d760ac442f0000027102c5e0aeabf96b58474ced1c9857cc30c";
    // Same tool on 9000 frames of 48 kHz mono 24-bit (`-b 4096`).
    const B: &str = "10001000000289000c780bb8017000002328b629aa2256d2c6dc8f1da643dd886598";

    fn a() -> StreamInfoBlock {
        StreamInfoBlock {
            min_block_size: 4096,
            max_block_size: 4096,
            min_frame_size: 1443,
            max_frame_size: 3446,
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
            total_samples: 10000,
            md5: hx("2c5e0aeabf96b58474ced1c9857cc30c").try_into().unwrap(),
        }
    }

    fn b() -> StreamInfoBlock {
        StreamInfoBlock {
            min_frame_size: 649,
            max_frame_size: 3192,
            sample_rate: 48000,
            channels: 1,
            bits_per_sample: 24,
            total_samples: 9000,
            md5: hx("b629aa2256d2c6dc8f1da643dd886598").try_into().unwrap(),
            ..a()
        }
    }

    #[test]
    fn parses_and_writes_real_blocks() {
        for (hex, want) in [(A, a()), (B, b())] {
            let bytes = hx(hex);
            assert_eq!(StreamInfoBlock::parse(&bytes), Ok(want));
            assert_eq!(want.write().to_vec(), bytes);
        }
    }

    fn malformed(r: Result<StreamInfoBlock>) {
        assert!(matches!(r, Err(Error::Malformed(_))), "{r:?}");
    }

    #[test]
    fn wrong_lengths_are_malformed() {
        let ok = hx(A);
        for n in 0..34 {
            malformed(StreamInfoBlock::parse(&ok[..n]));
        }
        let mut long = ok;
        long.push(0);
        malformed(StreamInfoBlock::parse(&long));
    }

    fn with(f: impl FnOnce(&mut StreamInfoBlock)) -> Vec<u8> {
        let mut s = a();
        f(&mut s);
        // Bypass the debug asserts: these fields are in range, only the combination is bad.
        s.write().to_vec()
    }

    #[test]
    fn invalid_values_are_malformed() {
        malformed(StreamInfoBlock::parse(&with(|s| s.sample_rate = 0)));
        malformed(StreamInfoBlock::parse(&with(|s| s.min_block_size = 15)));
        malformed(StreamInfoBlock::parse(&with(|s| s.max_block_size = 15)));
        malformed(StreamInfoBlock::parse(&with(|s| {
            s.min_block_size = 4097;
        })));
        malformed(StreamInfoBlock::parse(&with(|s| s.bits_per_sample = 3)));
        assert!(StreamInfoBlock::parse(&with(|s| s.bits_per_sample = 4)).is_ok());
        assert!(StreamInfoBlock::parse(&with(|s| s.min_block_size = 16)).is_ok());
    }

    #[test]
    fn unknown_values_are_zero() {
        let s = with(|s| {
            s.min_frame_size = 0;
            s.max_frame_size = 0;
            s.total_samples = 0;
            s.md5 = [0; 16];
        });
        let p = StreamInfoBlock::parse(&s).unwrap();
        assert_eq!(
            (p.min_frame_size, p.max_frame_size, p.total_samples, p.md5),
            (0, 0, 0, [0; 16])
        );
    }

    proptest! {
        #[test]
        fn write_then_parse_round_trips(
            min in 16u16..=u16::MAX,
            extra in any::<u16>(),
            min_fs in 0u32..1 << 24,
            max_fs in 0u32..1 << 24,
            rate in 1u32..1 << 20,
            channels in 1u8..=8,
            bits in 4u8..=32,
            total in 0u64..1 << 36,
            md5 in any::<[u8; 16]>(),
        ) {
            let s = StreamInfoBlock {
                min_block_size: min,
                max_block_size: min.saturating_add(extra),
                min_frame_size: min_fs,
                max_frame_size: max_fs,
                sample_rate: rate,
                channels,
                bits_per_sample: bits,
                total_samples: total,
                md5,
            };
            prop_assert_eq!(StreamInfoBlock::parse(&s.write()), Ok(s));
        }

        #[test]
        fn arbitrary_bodies_never_panic(body in prop::collection::vec(any::<u8>(), 0..40)) {
            let _ = StreamInfoBlock::parse(&body);
        }
    }
}
