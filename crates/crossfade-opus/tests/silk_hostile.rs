//! Hostile SILK frames through the public `Decoder`: any bytes after a SILK TOC (configs 0-11,
//! mono and stereo, codes 0-3), in any order, decode or fail cleanly: no panic (debug builds
//! trap on overflow), output of the packet's length, and finite. Not bounded by full scale: a
//! SILK frame with bytes to spare carries a redundant CELT frame (RFC 6716 4.5.1), and CELT
//! decodes noise to any band energy.

use crossfade_opus::{Decoder, Packet};
use proptest::prelude::*;

fn toc(config: u8, stereo: bool, code: u8) -> u8 {
    (config << 3) | (u8::from(stereo) << 2) | code
}

/// A packet of `code` around `payload`, valid often enough to reach SILK.
fn packet(config: u8, stereo: bool, code: u8, payload: &[u8], frames: u8) -> Vec<u8> {
    let mut p = vec![toc(config, stereo, code)];
    match code {
        0 | 1 => {}
        2 => {
            let first = payload.len() / 3;
            p.push(first.min(251) as u8);
        }
        _ => p.push(frames.clamp(1, 3)), // CBR, no padding
    }
    p.extend_from_slice(payload);
    p
}

fn check(d: &mut Decoder, data: &[u8]) {
    let mut out = vec![];
    if let Ok(n) = d.decode(Some(data), &mut out) {
        let p = Packet::parse(data).unwrap();
        assert_eq!(n, p.duration());
        assert_eq!(out.len(), n * d.channels());
        assert!(out.iter().all(|s| s.is_finite()));
    } else {
        assert!(out.is_empty());
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2000, ..ProptestConfig::default() })]

    #[test]
    fn any_silk_frame_decodes_or_fails_cleanly(
        channels in 1usize..=2,
        frames in proptest::collection::vec(
            (0u8..12, any::<bool>(), 0u8..4, proptest::collection::vec(any::<u8>(), 0..400), 1u8..4),
            1..12,
        ),
    ) {
        let mut d = Decoder::new(channels).unwrap();
        for (config, stereo, code, payload, n) in &frames {
            check(&mut d, &packet(*config, *stereo, *code, payload, *n));
        }
    }

    #[test]
    fn extreme_bytes_decode_or_fail_cleanly(
        channels in 1usize..=2,
        config in 0u8..12,
        stereo in any::<bool>(),
        fill in prop_oneof![Just(0u8), Just(0xFF), Just(0x80), Just(0x7F), any::<u8>()],
        len in 1usize..1200,
        repeat in 1usize..6,
    ) {
        let mut d = Decoder::new(channels).unwrap();
        let p = packet(config, stereo, 0, &vec![fill; len], 1);
        for _ in 0..repeat {
            check(&mut d, &p);
        }
    }
}
