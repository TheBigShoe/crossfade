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

    fn malformed(r: Result<StreamInfoBlock, Error>) {
        assert!(matches!(r, Err(Error::Malformed { .. })), "{r:?}");
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
        assert_eq!((p.min_frame_size, p.max_frame_size, p.total_samples, p.md5), (0, 0, 0, [0; 16]));
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
