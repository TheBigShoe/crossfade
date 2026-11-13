use super::*;
use crate::testing::packet_writer::{Packing, length_bytes, write_packet};
use proptest::prelude::*;

fn invalid(r: Result<Packet<'_>>) -> bool {
    matches!(r, Err(Error::InvalidPacket(_)))
}

#[test]
fn the_toc_byte_follows_table_2() {
    // In tenths of a millisecond: 48 samples a millisecond at 48 kHz.
    let ms = |c: u8| Toc(c << 3).frame_samples() * 10 / 48;
    for c in 0..32u8 {
        let toc = Toc(c << 3);
        let (mode, bw) = match c {
            0..=3 => (Mode::Silk, Bandwidth::Narrow),
            4..=7 => (Mode::Silk, Bandwidth::Medium),
            8..=11 => (Mode::Silk, Bandwidth::Wide),
            12..=13 => (Mode::Hybrid, Bandwidth::SuperWide),
            14..=15 => (Mode::Hybrid, Bandwidth::Full),
            16..=19 => (Mode::Celt, Bandwidth::Narrow),
            20..=23 => (Mode::Celt, Bandwidth::Wide),
            24..=27 => (Mode::Celt, Bandwidth::SuperWide),
            _ => (Mode::Celt, Bandwidth::Full),
        };
        assert_eq!((toc.config(), toc.mode(), toc.bandwidth()), (c, mode, bw));
        let tenths = match c {
            0..=11 => [100, 200, 400, 600][usize::from(c % 4)],
            12..=15 => [100, 200][usize::from(c % 2)],
            _ => [25, 50, 100, 200][usize::from(c % 4)],
        };
        assert_eq!(ms(c), tenths, "config {c}");
    }
    let toc = Toc(0b1111_1111);
    assert!(toc.stereo());
    assert_eq!(toc.code(), 3);
    assert!(!Toc(0b1111_1011).stereo());
}

#[test]
fn the_rfc_examples_parse() {
    // Figure 8: one NB mono 20 ms SILK frame.
    let p = Packet::parse(&[1 << 3, 0xAA, 0xBB]).unwrap();
    assert_eq!(p.toc().mode(), Mode::Silk);
    assert_eq!(p.toc().bandwidth(), Bandwidth::Narrow);
    assert_eq!(p.frames(), &[&[0xAA, 0xBB][..]]);
    assert_eq!(p.duration(), 960);
    // Figure 9: two FB mono 5 ms CELT frames of the same size.
    let p = Packet::parse(&[(29 << 3) | 1, 1, 2, 3, 4]).unwrap();
    assert_eq!(p.frames(), &[&[1, 2][..], &[3, 4][..]]);
    assert_eq!(p.duration(), 480);
    // Figure 10: two FB mono 20 ms Hybrid frames, VBR code 3.
    let p = Packet::parse(&[(15 << 3) | 3, 0b1000_0010, 1, 9, 8, 7]).unwrap();
    assert_eq!(p.toc().mode(), Mode::Hybrid);
    assert_eq!(p.frames(), &[&[9][..], &[8, 7][..]]);
    assert_eq!(p.duration(), 1920);
    // Figure 11: four FB stereo 20 ms CELT frames, CBR code 3.
    let p = Packet::parse(&[(31 << 3) | 0b111, 4, 1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
    assert!(p.toc().stereo());
    assert_eq!(p.frames().len(), 4);
    assert_eq!(p.frames()[3], &[7, 8]);
    assert_eq!(p.duration(), 3840);
}

#[test]
fn r1_a_packet_has_a_toc_byte() {
    assert!(invalid(Packet::parse(&[])));
    assert!(invalid(Packet::parse_self_delimited(&[])));
    // A lone TOC is a code 0 packet with an empty frame (DTX).
    assert_eq!(Packet::parse(&[0]).unwrap().frames(), &[&[][..]]);
}

#[test]
fn r2_no_implicit_frame_is_longer_than_1275_bytes() {
    let mut p = vec![0u8; 1276];
    assert_eq!(Packet::parse(&p).unwrap().frames()[0].len(), 1275);
    p.push(0);
    assert!(invalid(Packet::parse(&p)));
    // Code 1: each half.
    let mut p = vec![1u8; 1 + 2 * 1275];
    assert!(Packet::parse(&p).is_ok());
    p.extend([0, 0]);
    assert!(invalid(Packet::parse(&p)));
    // Code 2: the second frame.
    let mut p = vec![2u8, 0];
    p.extend(vec![0; 1276]);
    assert!(invalid(Packet::parse(&p)));
    // Code 3 CBR: each frame; VBR: the last.
    let mut p = vec![3u8, 1];
    p.extend(vec![0; 1276]);
    assert!(invalid(Packet::parse(&p)));
    let mut p = vec![3u8, 0x82, 0];
    p.extend(vec![0; 1276]);
    assert!(invalid(Packet::parse(&p)));
}

#[test]
fn r3_code_1_payloads_are_even() {
    assert!(invalid(Packet::parse(&[1, 0])));
    assert!(invalid(Packet::parse(&[1, 0, 0, 0])));
    assert_eq!(Packet::parse(&[1]).unwrap().frames(), &[&[][..], &[][..]]);
}

#[test]
fn r4_code_2_lengths_fit() {
    assert!(invalid(Packet::parse(&[2])));
    for b in 252..=255 {
        assert!(invalid(Packet::parse(&[2, b])));
    }
    // The only valid two-byte code 2 packet: both frames empty.
    assert_eq!(Packet::parse(&[2, 0]).unwrap().frames(), &[&[][..], &[][..]]);
    assert!(invalid(Packet::parse(&[2, 1])));
    assert!(invalid(Packet::parse(&[2, 3, 1, 2])));
    let p = Packet::parse(&[2, 2, 1, 2, 3]).unwrap();
    assert_eq!(p.frames(), &[&[1, 2][..], &[3][..]]);
    // Two-byte length: 252 + 4 * 1 = 256.
    let mut p = vec![2u8, 252, 1];
    p.extend(vec![7; 256 + 3]);
    let parsed = Packet::parse(&p).unwrap();
    assert_eq!(parsed.frames()[0].len(), 256);
    assert_eq!(parsed.frames()[1].len(), 3);
}

#[test]
fn r5_code_3_holds_one_frame_to_120_ms() {
    // M = 0.
    assert!(invalid(Packet::parse(&[3, 0])));
    // 48 frames of 2.5 ms (CELT config 16), but not 49.
    let p = Packet::parse(&[(16 << 3) | 3, 48]).unwrap();
    assert_eq!(p.frames().len(), 48);
    assert_eq!(p.duration(), 5760);
    assert!(invalid(Packet::parse(&[(16 << 3) | 3, 49])));
    // Two 60 ms SILK frames, but not three; six 20 ms frames, but not seven.
    assert!(Packet::parse(&[(3 << 3) | 3, 2]).is_ok());
    assert!(invalid(Packet::parse(&[(3 << 3) | 3, 3])));
    assert!(Packet::parse(&[(1 << 3) | 3, 6]).is_ok());
    assert!(invalid(Packet::parse(&[(1 << 3) | 3, 7])));
}

#[test]
fn r6_cbr_code_3_divides_evenly_after_padding() {
    assert!(invalid(Packet::parse(&[3])));
    // Two frames in 5 bytes: not a multiple.
    assert!(invalid(Packet::parse(&[3, 2, 1, 2, 3, 4, 5])));
    assert_eq!(
        Packet::parse(&[3, 2, 1, 2, 3, 4]).unwrap().frames(),
        &[&[1, 2][..], &[3, 4][..]]
    );
    // Padding of 1 (one length byte plus one byte) leaves 4: two frames of 2.
    let p = Packet::parse(&[3, 0x42, 1, 1, 2, 3, 4, 0xEE]).unwrap();
    assert_eq!(p.frames(), &[&[1, 2][..], &[3, 4][..]]);
    assert_eq!(p.padding(), &[0xEE]);
    // Padding longer than the packet.
    assert!(invalid(Packet::parse(&[3, 0x41, 5, 0, 0])));
    // The padding flag with no length byte.
    assert!(invalid(Packet::parse(&[3, 0x41])));
    // 255 asks for another length byte.
    assert!(invalid(Packet::parse(&[3, 0x41, 255])));
}

#[test]
fn r7_vbr_code_3_holds_its_lengths_and_padding() {
    // Three frames, lengths 1 and 2 given, the last takes the rest.
    let p = Packet::parse(&[3, 0x83, 1, 2, 9, 8, 8, 7, 7, 7]).unwrap();
    assert_eq!(p.frames(), &[&[9][..], &[8, 8][..], &[7, 7, 7][..]]);
    // A length past the end.
    assert!(invalid(Packet::parse(&[3, 0x83, 1, 9, 9])));
    // Lengths missing.
    assert!(invalid(Packet::parse(&[3, 0x83, 1])));
    // Lengths that fit but leave less than nothing for the last frame.
    assert!(invalid(Packet::parse(&[3, 0x83, 2, 2, 1, 1, 1])));
    // With padding: the lengths and the padding both fit.
    let p = Packet::parse(&[3, 0xC2, 2, 1, 5, 6, 6, 0, 0]).unwrap();
    assert_eq!(p.frames(), &[&[5][..], &[6, 6][..]]);
    assert_eq!(p.padding().len(), 2);
    assert!(invalid(Packet::parse(&[3, 0xC2, 3, 2, 5, 6, 0, 0])));
}

#[test]
fn padding_counts_never_overflow() {
    // RFC 8251 section 4: a padding length beyond the packet is an error, however many 255s.
    let mut p = vec![3u8, 0x41];
    p.extend(vec![255; 100_000]);
    p.push(0);
    assert!(invalid(Packet::parse(&p)));
    // Padding of 254 * 2 + 3 = 511 bytes, exactly filling the packet.
    let mut p = vec![3u8, 0x41, 255, 255, 3];
    p.extend(vec![0; 511]);
    let parsed = Packet::parse(&p).unwrap();
    assert_eq!(parsed.frames(), &[&[][..]]);
    assert_eq!(parsed.padding().len(), 511);
}

#[test]
fn self_delimited_packets_carry_the_last_length() {
    // Code 0: the length, then the frame; what follows belongs to the next stream.
    let p = Packet::parse_self_delimited(&[0, 2, 7, 7, 0xFF, 0xFF]).unwrap();
    assert_eq!(p.frames(), &[&[7, 7][..]]);
    assert_eq!(p.byte_len(), 4);
    // Code 1: one length for both.
    let p = Packet::parse_self_delimited(&[1, 1, 7, 8, 0xFF]).unwrap();
    assert_eq!(p.frames(), &[&[7][..], &[8][..]]);
    assert_eq!(p.byte_len(), 4);
    // Code 2: N1 then N2.
    let p = Packet::parse_self_delimited(&[2, 1, 2, 7, 8, 8, 0xFF]).unwrap();
    assert_eq!(p.frames(), &[&[7][..], &[8, 8][..]]);
    assert_eq!(p.byte_len(), 6);
    // CBR code 3: one length for all, then padding.
    let p = Packet::parse_self_delimited(&[3, 0x43, 1, 1, 7, 8, 9, 0, 0xFF]).unwrap();
    assert_eq!(p.frames(), &[&[7][..], &[8][..], &[9][..]]);
    assert_eq!(p.padding().len(), 1);
    assert_eq!(p.byte_len(), 8);
    // VBR code 3: the last length after the others.
    let p = Packet::parse_self_delimited(&[3, 0x82, 1, 2, 7, 8, 8, 0xFF]).unwrap();
    assert_eq!(p.frames(), &[&[7][..], &[8, 8][..]]);
    assert_eq!(p.byte_len(), 7);
    // Lengths past the end.
    assert!(invalid(Packet::parse_self_delimited(&[0])));
    assert!(invalid(Packet::parse_self_delimited(&[0, 3, 1, 1])));
    assert!(invalid(Packet::parse_self_delimited(&[1, 2, 1, 1, 1])));
    assert!(invalid(Packet::parse_self_delimited(&[3, 0x03, 2, 1, 1, 1, 1, 1])));
    assert!(invalid(Packet::parse_self_delimited(&[2, 1, 3, 7, 8, 8])));
}

#[test]
fn packet_duration_needs_only_the_toc_and_count() {
    // Frames need not be valid: the Ogg reader times packets without parsing them.
    assert_eq!(packet_duration(&[1 << 3]), Ok(960));
    assert_eq!(packet_duration(&[(1 << 3) | 1, 0, 0, 0]), Ok(1920));
    assert_eq!(packet_duration(&[(1 << 3) | 2, 255]), Ok(1920));
    assert_eq!(packet_duration(&[(16 << 3) | 3, 48]), Ok(5760));
    assert!(matches!(packet_duration(&[]), Err(Error::InvalidPacket(_))));
    assert!(matches!(packet_duration(&[3]), Err(Error::InvalidPacket(_))));
    assert!(matches!(packet_duration(&[3, 0]), Err(Error::InvalidPacket(_))));
    assert!(matches!(
        packet_duration(&[(1 << 3) | 3, 7]),
        Err(Error::InvalidPacket(_))
    ));
}

#[test]
fn bandwidth_names_the_celt_end_band() {
    // opus_decoder.c: NB 13, MB and WB 17, SWB 19, FB 21.
    let ends = [
        Bandwidth::Narrow,
        Bandwidth::Medium,
        Bandwidth::Wide,
        Bandwidth::SuperWide,
        Bandwidth::Full,
    ]
    .map(Bandwidth::celt_end_band);
    assert_eq!(ends, [13, 17, 17, 19, 21]);
}

fn frames_and_packing() -> impl Strategy<Value = (u8, bool, Vec<Vec<u8>>, Packing)> {
    let frame = |max: usize| proptest::collection::vec(any::<u8>(), 0..=max);
    let config_and_stereo = (0u8..32, any::<bool>());
    let one = frame(1275).prop_map(|f| (vec![f], Packing::One));
    let two_equal = (0usize..=300)
        .prop_flat_map(move |n| {
            (
                proptest::collection::vec(any::<u8>(), n),
                proptest::collection::vec(any::<u8>(), n),
            )
        })
        .prop_map(|(a, b)| (vec![a, b], Packing::TwoEqual));
    let two_different =
        (frame(1275), frame(1275)).prop_map(|(a, b)| (vec![a, b], Packing::TwoDifferent));
    let vbr = (proptest::collection::vec(frame(300), 1..=6), proptest::option::of(0usize..700))
        .prop_map(|(f, padding)| (f, Packing::Signalled { vbr: true, padding }));
    let cbr = (1usize..=6, 0usize..=100, proptest::option::of(0usize..700))
        .prop_map(|(m, n, padding)| {
            let f = (0..m).map(|i| vec![i as u8; n]).collect();
            (f, Packing::Signalled { vbr: false, padding })
        });
    (config_and_stereo, prop_oneof![one, two_equal, two_different, vbr, cbr]).prop_filter_map(
        "within 120 ms",
        |((config, stereo), (frames, packing))| {
            let total = Toc(config << 3).frame_samples() * frames.len();
            (total <= 5760).then_some((config, stereo, frames, packing))
        },
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn written_packets_parse_back(
        (config, stereo, frames, packing) in frames_and_packing(),
        trailer in proptest::collection::vec(any::<u8>(), 0..8),
    ) {
        let slices: Vec<&[u8]> = frames.iter().map(Vec::as_slice).collect();
        let packet = write_packet(config, stereo, &slices, packing, false);
        let p = Packet::parse(&packet).unwrap();
        prop_assert_eq!(p.toc().config(), config);
        prop_assert_eq!(p.toc().stereo(), stereo);
        prop_assert_eq!(p.frames(), &slices[..]);
        prop_assert_eq!(p.byte_len(), packet.len());
        prop_assert_eq!(p.duration(), Toc(config << 3).frame_samples() * frames.len());
        prop_assert_eq!(packet_duration(&packet), Ok(p.duration()));
        if let Packing::Signalled { padding, .. } = packing {
            prop_assert_eq!(p.padding().len(), padding.unwrap_or(0));
        }
        // Self-delimited, followed by another stream's bytes.
        let mut delimited = write_packet(config, stereo, &slices, packing, true);
        let len = delimited.len();
        delimited.extend(&trailer);
        let p = Packet::parse_self_delimited(&delimited).unwrap();
        prop_assert_eq!(p.frames(), &slices[..]);
        prop_assert_eq!(p.byte_len(), len);
    }

    #[test]
    fn any_bytes_parse_or_fail_cleanly(bytes in proptest::collection::vec(any::<u8>(), 0..300)) {
        for p in [Packet::parse(&bytes), Packet::parse_self_delimited(&bytes)].into_iter().flatten() {
            prop_assert!((1..=MAX_FRAMES).contains(&p.frames().len()));
            prop_assert!(p.duration() <= MAX_PACKET_SAMPLES);
            prop_assert!(p.byte_len() <= bytes.len());
            let inside: usize = p.frames().iter().map(|f| f.len()).sum::<usize>() + p.padding().len();
            prop_assert!(inside < p.byte_len());
            for f in p.frames() {
                prop_assert!(f.len() <= MAX_FRAME_LEN);
            }
        }
        let _ = packet_duration(&bytes);
    }

    #[test]
    fn length_bytes_round_trip(len in 0usize..=1275) {
        let mut packet = vec![2u8];
        packet.extend(length_bytes(len));
        packet.extend(vec![0; len]);
        prop_assert_eq!(Packet::parse(&packet).unwrap().frames()[0].len(), len);
    }
}
