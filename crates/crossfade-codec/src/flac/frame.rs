#[cfg(test)]
mod tests {
    use super::*;
    use crate::flac::crc::crc8;
    use proptest::prelude::*;
    use ChannelAssignment::{Independent, LeftSide, MidSide, SideRight};

    fn hx(s: &str) -> Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn hdr(
        variable: bool,
        block_size: u32,
        sample_rate: Option<u32>,
        channels: ChannelAssignment,
        bits: Option<u8>,
        number: u64,
        len: usize,
    ) -> FrameHeader {
        FrameHeader {
            variable_block_size: variable,
            block_size,
            sample_rate,
            channels,
            bits_per_sample: bits,
            number,
            header_len: len,
        }
    }

    // Vectors from the reference encoder, flac 1.5.0 (`/opt/homebrew/bin/flac -s --no-padding
    // [--lax] -b N`), run on a generated sine: WAV input for the 1- and 2-channel cases, raw input
    // (`--force-raw-format --endian=little --sign=signed --channels=C --bps=B --sample-rate=R`)
    // for the others. Each header was cut from the output at a sync code whose CRC-8 verifies.
    // The expected fields come from how the input was generated, not from this parser.
    //
    // Hand-assembled vectors (marked) cover what the reference encoder never emits (variable
    // block size, 31- and 36-bit numbers, rare codes): laid out per RFC 9639 section 9.1 with an
    // independent bitwise CRC-8 in a throwaway script.
    fn vectors() -> Vec<(&'static str, &'static str, FrameHeader)> {
        vec![
            // 10000 frames, 44.1 kHz stereo 16-bit, -b 4096; mid/side was chosen by the encoder.
            ("a0", "fff8c9a8008d", hdr(false, 4096, Some(44100), MidSide, Some(16), 0, 6)),
            // Same file, last frame: 1808 samples left over, left/side.
            ("a2", "fff8798802070fc0", hdr(false, 1808, Some(44100), LeftSide, Some(16), 2, 8)),
            // 9000 frames, 48 kHz mono 24-bit, -b 4096.
            ("b0", "fff8ca0c007c", hdr(false, 4096, Some(48000), Independent(1), Some(24), 0, 6)),
            ("b2", "fff87a0c02032783", hdr(false, 808, Some(48000), Independent(1), Some(24), 2, 8)),
            // 1000 frames, 8 kHz mono 8-bit, -b 256; the last frame has 232 samples.
            ("c0", "fff884020000", hdr(false, 256, Some(8000), Independent(1), Some(8), 0, 6)),
            ("c3", "fff8640203e7e0", hdr(false, 232, Some(8000), Independent(1), Some(8), 3, 7)),
            // 3000 frames, 96 kHz stereo 32-bit, -b 1152.
            ("d0", "fff83b8e00e7", hdr(false, 1152, Some(96000), LeftSide, Some(32), 0, 6)),
            ("d2", "fff87b1e0202b746", hdr(false, 696, Some(96000), Independent(2), Some(32), 2, 8)),
            // 3000 frames, 22.05 kHz stereo 16-bit, --lax -b 100 (8-bit block size field).
            ("e0", "fff86618006343", hdr(false, 100, Some(22050), Independent(2), Some(16), 0, 7)),
            ("e29", "fff866181d63fd", hdr(false, 100, Some(22050), Independent(2), Some(16), 29, 7)),
            // 5000 frames, 11.025 kHz mono 16-bit, --lax -b 1000 (16-bit field) and a 16-bit
            // Hz sample-rate field.
            ("f0", "fff87d080003e72b1139", hdr(false, 1000, Some(11025), Independent(1), Some(16), 0, 10)),
            ("f4", "fff87d080403e72b11b6", hdr(false, 1000, Some(11025), Independent(1), Some(16), 4, 10)),
            // 3000 frames, 32 kHz six channels 16-bit (raw), -b 576.
            ("g0", "fff82858003c", hdr(false, 576, Some(32000), Independent(6), Some(16), 0, 6)),
            // 2000 frames, 192 kHz eight channels 24-bit (raw), -b 192; last frame has 80.
            ("h0", "fff8137c00cb", hdr(false, 192, Some(192000), Independent(8), Some(24), 0, 6)),
            ("h10", "fff8637c0a4f25", hdr(false, 80, Some(192000), Independent(8), Some(24), 10, 7)),
            // 6000 frames, 44.1 kHz mono 16-bit, --lax -b 16: 375 frames, so a two-byte number.
            ("i0", "fff86908000f30", hdr(false, 16, Some(44100), Independent(1), Some(16), 0, 7)),
            ("i374", "fff86908c5b60f02", hdr(false, 16, Some(44100), Independent(1), Some(16), 374, 8)),
            // 2500 frames, 12345 Hz stereo 16-bit, -b 1152 (16-bit Hz field).
            ("j0", "fff83d1800303917", hdr(false, 1152, Some(12345), Independent(2), Some(16), 0, 8)),
            // 2500 frames, 88.2 kHz stereo 16-bit, -b 1152.
            ("k0", "fff8311800ff", hdr(false, 1152, Some(88200), Independent(2), Some(16), 0, 6)),
            // 100 frames at 1 Hz mono 16-bit, --lax -b 32.
            ("l0", "fff86d08001f0001f8", hdr(false, 32, Some(1), Independent(1), Some(16), 0, 9)),
            // 3000 frames, 70010 Hz mono 16-bit (raw), --lax -b 1152 (tens-of-Hz field).
            ("m0", "fff83e08001b59c8", hdr(false, 1152, Some(70010), Independent(1), Some(16), 0, 8)),
            // 3000 frames, 700001 Hz stereo 16-bit (raw), --lax: not codable, so "from STREAMINFO".
            ("n0", "fff830180094", hdr(false, 1152, None, Independent(2), Some(16), 0, 6)),
            // 2000 frames, 44.1 kHz three channels 16-bit (raw), --lax -b 4608: one short frame.
            ("o0", "fff879280007cfa7", hdr(false, 2000, Some(44100), Independent(3), Some(16), 0, 7)),
            // 70000 frames, 8 kHz mono 24-bit (raw), --lax -b 65535; second frame has 4465.
            ("p0", "fff8740c00fffeee", hdr(false, 65535, Some(8000), Independent(1), Some(24), 0, 8)),
            ("p1", "fff8740c011170b3", hdr(false, 4465, Some(8000), Independent(1), Some(24), 1, 8)),
            // Hand-assembled: variable, 65536 samples, 36-bit sample number, rate and depth from
            // STREAMINFO.
            ("v1", "fff97010febfbfbfbfbfbfffff9e", hdr(true, 65536, None, Independent(2), None, (1 << 36) - 1, 14)),
            // Hand-assembled: fixed, largest 31-bit frame number, six-byte number.
            ("v2", "fff88908fdbfbfbfbfbfc7", hdr(false, 256, Some(44100), Independent(1), Some(16), (1 << 31) - 1, 11)),
            // Hand-assembled: variable, 255 kHz in the kHz field, 36-bit number 0x123456789.
            ("v3", "fff99c8cfe84a391969e89ff23", hdr(true, 512, Some(255_000), LeftSide, Some(24), 0x1_2345_6789, 13)),
            // Hand-assembled: variable, small number, 8-bit depth, mid/side.
            ("v4", "fff91aa20590", hdr(true, 192, Some(48000), MidSide, Some(8), 5, 6)),
            // Hand-assembled: the first two-byte and three-byte numbers.
            ("v5", "fff81000c28096", hdr(false, 192, None, Independent(1), None, 128, 7)),
            ("v6", "fff81000e0a08059", hdr(false, 192, None, Independent(1), None, 2048, 8)),
            // Hand-assembled: tens-of-Hz field at its maximum (655350 Hz), 20-bit, four-byte number.
            ("v7", "fff94e2af092bf91ffff1d", hdr(true, 2304, Some(655_350), Independent(3), Some(20), 77777, 11)),
            // Hand-assembled: 255 kHz kHz field, 12-bit.
            ("v8", "fff83c3403ff78", hdr(false, 1152, Some(255_000), Independent(4), Some(12), 3, 7)),
            // Hand-assembled: 8-bit block-size field holding 0, so one sample; 12-bit.
            ("v9", "fff86914000045", hdr(false, 1, Some(44100), Independent(2), Some(12), 0, 7)),
            // Hand-assembled: 32768 samples, 88.2 kHz, side/right, 20-bit.
            ("v10", "fff8f19a01e9", hdr(false, 32768, Some(88200), SideRight, Some(20), 1, 6)),
        ]
    }

    #[test]
    fn parses_every_vector() {
        for (name, bytes, want) in vectors() {
            let bytes = hx(bytes);
            assert_eq!(parse_header(&bytes), Ok(want), "{name}");
            // Whatever follows the header (the first subframe) is not read.
            let mut more = bytes.clone();
            more.extend([0xFF, 0xF8, 0x00, 0x12]);
            assert_eq!(parse_header(&more), Ok(want), "{name} with a payload");
        }
    }

    #[test]
    fn writes_every_vector_back_byte_for_byte() {
        for (name, bytes, want) in vectors() {
            let mut out = vec![0xEE]; // appends, never clears
            write_header(&want, &mut out);
            assert_eq!(&out[1..], hx(bytes), "{name}");
        }
    }

    #[test]
    fn a_header_cut_short_needs_more_data_at_every_length() {
        for (name, bytes, want) in vectors() {
            let bytes = hx(bytes);
            for n in 0..want.header_len {
                assert_eq!(parse_header(&bytes[..n]), Err(HeaderError::NeedMoreData), "{name} {n}");
            }
        }
    }

    fn is_invalid(r: Result<FrameHeader, HeaderError>) -> bool {
        matches!(r, Err(HeaderError::Invalid(_)))
    }

    /// A one-byte-number header with a correct CRC-8 around the given raw fields.
    fn build(second: u8, byte2: u8, byte3: u8, tail: &[u8]) -> Vec<u8> {
        let mut v = vec![0xFF, second, byte2, byte3];
        v.extend(tail);
        v.push(crc8(&v));
        v
    }

    #[test]
    fn the_builder_agrees_with_a_real_vector() {
        assert_eq!(build(0xF8, 0xC9, 0xA8, &[0]), hx("fff8c9a8008d"));
    }

    #[test]
    fn reserved_and_invalid_codes_are_rejected() {
        // Block size code 0 is reserved.
        assert!(is_invalid(parse_header(&build(0xF8, 0x09, 0x08, &[0]))));
        // Sample rate code 15 is invalid.
        assert!(is_invalid(parse_header(&build(0xF8, 0xCF, 0x08, &[0]))));
        // Sample size code 3 is reserved.
        assert!(is_invalid(parse_header(&build(0xF8, 0xC9, 0x06, &[0]))));
        // The bit after the sample size is reserved.
        assert!(is_invalid(parse_header(&build(0xF8, 0xC9, 0x09, &[0]))));
        // Channel assignments 11 to 15 are reserved.
        for code in 11..=15u8 {
            assert!(is_invalid(parse_header(&build(0xF8, 0xC9, code << 4 | 0x08, &[0]))), "{code}");
        }
        // The ten assignments that are valid are accepted.
        for code in 0..=10u8 {
            assert!(parse_header(&build(0xF8, 0xC9, code << 4 | 0x08, &[0])).is_ok(), "{code}");
        }
        // A zero sample rate in each explicit field.
        assert!(is_invalid(parse_header(&build(0xF8, 0xCC, 0x08, &[0, 0]))));
        assert!(is_invalid(parse_header(&build(0xF8, 0xCD, 0x08, &[0, 0, 0]))));
        assert!(is_invalid(parse_header(&build(0xF8, 0xCE, 0x08, &[0, 0, 0]))));
    }

    #[test]
    fn a_wrong_sync_code_is_rejected_before_anything_else() {
        for second in 0..=255u8 {
            let r = parse_header(&build(second, 0xC9, 0xA8, &[0]));
            if second == 0xF8 || second == 0xF9 {
                assert!(r.is_ok(), "{second:#x}");
            } else {
                assert!(is_invalid(r), "{second:#x}");
            }
        }
        assert!(is_invalid(parse_header(&[0xFE, 0xF8])));
        // A bad first byte needs no further data to be known bad.
        assert!(is_invalid(parse_header(&[0x00])));
        assert!(is_invalid(parse_header(&[0xFF, 0x00])));
    }

    #[test]
    fn a_bad_crc_is_rejected() {
        let good = hx("fff8c9a8008d");
        for crc in 0..=255u8 {
            let mut b = good.clone();
            b[5] = crc;
            assert_eq!(is_invalid(parse_header(&b)), crc != 0x8d, "{crc:#x}");
        }
        // A changed field the CRC covers.
        let mut b = good;
        b[4] = 0x01;
        assert!(is_invalid(parse_header(&b)));
    }

    #[test]
    fn malformed_coded_numbers_are_rejected() {
        let with_number = |second: u8, number: &[u8]| build(second, 0xC9, 0xA8, number);
        // 0xFF is never a lead byte; 0x80 to 0xBF is a continuation byte.
        assert!(is_invalid(parse_header(&with_number(0xF8, &[0xFF]))));
        assert!(is_invalid(parse_header(&with_number(0xF8, &[0x80]))));
        assert!(is_invalid(parse_header(&with_number(0xF8, &[0xBF]))));
        // A continuation byte that is not 10xxxxxx.
        assert!(is_invalid(parse_header(&with_number(0xF8, &[0xC2, 0x00]))));
        assert!(is_invalid(parse_header(&with_number(0xF8, &[0xC2, 0xC0]))));
        // Overlong forms of values that fit a shorter one.
        assert!(is_invalid(parse_header(&with_number(0xF8, &[0xC0, 0x80]))));
        assert!(is_invalid(parse_header(&with_number(0xF8, &[0xC1, 0xBF]))));
        assert!(is_invalid(parse_header(&with_number(0xF8, &[0xE0, 0x9F, 0xBF]))));
        // A fixed-block-size frame number has 31 bits at most; the seven-byte form is for
        // variable block size only.
        let seven = [0xFE, 0x82, 0x80, 0x80, 0x80, 0x80, 0x80];
        assert!(is_invalid(parse_header(&with_number(0xF8, &seven))));
        assert!(parse_header(&with_number(0xF9, &seven)).is_ok());
        let six = [0xFD, 0x80, 0x80, 0x80, 0x80, 0x80]; // exactly 2^30: fine
        assert!(parse_header(&with_number(0xF8, &six)).is_ok());
    }

    #[test]
    fn first_sample_follows_the_blocking_strategy() {
        let fixed = hdr(false, 4096, None, Independent(1), None, 3, 6);
        let variable = hdr(true, 4096, None, Independent(1), None, 3, 6);
        assert_eq!(fixed.first_sample(4096), 3 * 4096);
        assert_eq!(fixed.first_sample(1152), 3 * 1152);
        assert_eq!(variable.first_sample(4096), 3);
        let big = hdr(false, 1, None, Independent(1), None, (1 << 31) - 1, 6);
        assert_eq!(big.first_sample(65535), ((1u64 << 31) - 1) * 65535);
    }

    #[test]
    fn channel_counts() {
        for n in 1..=8 {
            assert_eq!(Independent(n).channels(), n);
        }
        for c in [LeftSide, SideRight, MidSide] {
            assert_eq!(c.channels(), 2);
        }
    }

    #[test]
    fn writer_picks_the_shortest_codes() {
        let w = |block_size, sample_rate| {
            let mut out = Vec::new();
            write_header(&hdr(false, block_size, sample_rate, Independent(1), Some(16), 0, 0), &mut out);
            out
        };
        // Table block sizes use no extra bytes.
        for (size, code) in [(192, 1), (576, 2), (1152, 3), (2304, 4), (4608, 5), (256, 8), (512, 9), (1024, 10), (2048, 11), (4096, 12), (8192, 13), (16384, 14), (32768, 15)] {
            assert_eq!(w(size, None)[2] >> 4, code, "{size}");
            assert_eq!(w(size, None).len(), 6, "{size}");
        }
        assert_eq!(w(1, None)[2] >> 4, 6);
        assert_eq!(w(255, None)[2] >> 4, 6);
        assert_eq!(w(257, None)[2] >> 4, 7);
        assert_eq!(w(65536, None)[2] >> 4, 7);
        // Table rates, then kHz, then Hz, then tens of Hz.
        for (rate, code) in [(88200, 1), (176_400, 2), (192_000, 3), (8000, 4), (16000, 5), (22050, 6), (24000, 7), (32000, 8), (44100, 9), (48000, 10), (96000, 11), (12000, 12), (255_000, 12), (12_345, 13), (65535, 13), (70_010, 14), (655_350, 14)] {
            assert_eq!(w(4096, Some(rate))[2] & 15, code, "{rate}");
        }
        assert_eq!(w(4096, None)[2] & 15, 0);
    }

    #[test]
    fn depth_codes() {
        let w = |bits| {
            let mut out = Vec::new();
            write_header(&hdr(false, 4096, None, Independent(1), bits, 0, 0), &mut out);
            (out[3] >> 1) & 7
        };
        assert_eq!(
            [w(None), w(Some(8)), w(Some(12)), w(Some(16)), w(Some(20)), w(Some(24)), w(Some(32))],
            [0, 1, 2, 4, 5, 6, 7]
        );
    }

    fn arb_rate() -> impl Strategy<Value = Option<u32>> {
        prop_oneof![
            Just(None),
            prop::sample::select(vec![88200u32, 176_400, 192_000, 8000, 16000, 22050, 24000, 32000, 44100, 48000, 96000]).prop_map(Some),
            (1u32..=255).prop_map(|k| Some(k * 1000)),
            (1u32..=65535).prop_map(Some),
            (1u32..=65535).prop_map(|t| Some(t * 10)),
        ]
    }

    fn arb_channels() -> impl Strategy<Value = ChannelAssignment> {
        prop_oneof![
            (1u8..=8).prop_map(Independent),
            Just(LeftSide),
            Just(SideRight),
            Just(MidSide),
        ]
    }

    proptest! {
        #[test]
        fn write_then_parse_round_trips(
            variable in any::<bool>(),
            block_size in 1u32..=65536,
            sample_rate in arb_rate(),
            channels in arb_channels(),
            bits in prop::sample::select(vec![None, Some(8u8), Some(12), Some(16), Some(20), Some(24), Some(32)]),
            wide in any::<u64>(),
            tail in prop::collection::vec(any::<u8>(), 0..8),
        ) {
            let number = if variable { wide % (1 << 36) } else { wide % (1 << 31) };
            let mut h = hdr(variable, block_size, sample_rate, channels, bits, number, 0);
            let mut out = Vec::new();
            write_header(&h, &mut out);
            h.header_len = out.len();
            out.extend(&tail);
            prop_assert_eq!(parse_header(&out), Ok(h));
            // And every strict prefix of the header is just short.
            for n in 0..h.header_len {
                prop_assert_eq!(parse_header(&out[..n]), Err(HeaderError::NeedMoreData));
            }
        }

        #[test]
        fn arbitrary_bytes_never_panic_or_overrun(data in prop::collection::vec(any::<u8>(), 0..40)) {
            if let Ok(h) = parse_header(&data) {
                prop_assert!(h.header_len <= data.len());
            }
        }

        #[test]
        fn arbitrary_bytes_after_a_sync_never_panic_or_overrun(data in prop::collection::vec(any::<u8>(), 0..40)) {
            let mut v = vec![0xFF, 0xF8];
            v.extend(data);
            if let Ok(h) = parse_header(&v) {
                prop_assert!(h.header_len <= v.len());
            }
        }
    }
}
