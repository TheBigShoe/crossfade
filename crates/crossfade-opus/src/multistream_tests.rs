use super::*;
use crate::testing::packet_writer::self_delimited;
use crate::{Decoder, packet_duration};

fn noise(seed: u32, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x >> 24) as u8
        })
        .collect()
}

fn packet(config: u8, stereo: bool, seed: u32, len: usize) -> Vec<u8> {
    let mut p = vec![(config << 3) | (u8::from(stereo) << 2)];
    p.extend(noise(seed, len));
    p
}

fn mapping(family: u8, streams: u8, coupled: u8, table: &[u8]) -> ChannelMapping {
    ChannelMapping {
        family,
        streams,
        coupled,
        table: table.to_vec(),
    }
}

/// Every stream's packet but the last self-delimited, then the last as it is.
fn join(packets: &[Vec<u8>]) -> Vec<u8> {
    let (last, rest) = packets.split_last().unwrap();
    let mut out = vec![];
    for p in rest {
        out.extend(self_delimited(p));
    }
    out.extend_from_slice(last);
    out
}

/// What each stream decodes to on its own, `[stereo..., mono...]`.
fn alone(packets: &[Vec<u8>], coupled: usize) -> Vec<Vec<f32>> {
    packets
        .iter()
        .enumerate()
        .map(|(s, p)| {
            let mut d = Decoder::new(if s < coupled { 2 } else { 1 }).unwrap();
            let mut out = vec![];
            d.decode(Some(p), &mut out).unwrap();
            out
        })
        .collect()
}

#[test]
fn family_0_is_one_stream() {
    for channels in [1u8, 2] {
        let m = mapping(0, 1, channels - 1, &(0..channels).collect::<Vec<_>>());
        let mut ms = MultistreamDecoder::new(&m, usize::from(channels)).unwrap();
        let p = packet(15, true, 4, 90);
        let mut out = vec![];
        assert_eq!(ms.decode(Some(&p), &mut out), Ok(960));
        assert_eq!(out, alone(&[p], usize::from(channels) - 1)[0]);
        assert_eq!(ms.final_ranges().len(), 1);
        assert_ne!(ms.final_ranges()[0], 0);
    }
}

#[test]
fn channels_are_routed_by_the_table_and_255_is_silence() {
    // Two coupled streams and two mono ones (6 decoded channels), routed out of order, one
    // decoded channel twice and one output silent.
    let table = [4, 0, 1, 255, 5, 3, 2, 0];
    let m = mapping(1, 4, 2, &table);
    let packets = vec![
        packet(31, true, 1, 120),
        packet(15, true, 2, 100),
        packet(9, false, 3, 60),
        packet(28, false, 4, 40),
    ];
    // The 10 ms CELT packet is twice as short: give it two frames.
    let mut packets = packets;
    packets[3] = {
        let mut p = vec![(30 << 3) | 1];
        p.extend(noise(9, 80));
        p
    };
    for p in &packets {
        assert_eq!(packet_duration(p), Ok(960));
    }
    let want = alone(&packets, 2);
    let mut ms = MultistreamDecoder::new(&m, table.len()).unwrap();
    let mut out = vec![];
    assert_eq!(ms.decode(Some(&join(&packets)), &mut out), Ok(960));
    assert_eq!(out.len(), 960 * table.len());
    for (c, &t) in table.iter().enumerate() {
        let got: Vec<f32> = out.iter().skip(c).step_by(table.len()).copied().collect();
        let expected: Vec<f32> = match t {
            255 => vec![0.0; 960],
            0..=3 => want[usize::from(t / 2)]
                .iter()
                .skip(usize::from(t % 2))
                .step_by(2)
                .copied()
                .collect(),
            _ => want[usize::from(t) - 2].clone(),
        };
        assert_eq!(got, expected, "channel {c}");
    }
    assert_eq!(ms.final_ranges().len(), 4);
}

#[test]
fn a_lost_packet_conceals_every_stream_and_reset_forgets() {
    let m = mapping(1, 2, 1, &[0, 1, 2]);
    let mut ms = MultistreamDecoder::new(&m, 3).unwrap();
    let mut out = vec![];
    assert_eq!(ms.decode(None, &mut out), Ok(0));
    let packets = vec![packet(1, true, 1, 50), packet(1, false, 2, 50)];
    assert_eq!(ms.decode(Some(&join(&packets)), &mut out), Ok(960));
    out.clear();
    assert_eq!(ms.decode(None, &mut out), Ok(960));
    assert_eq!(out.len(), 960 * 3);
    assert!(out.iter().all(|x| x.is_finite()));
    out.clear();
    assert_eq!(ms.decode_fec(&join(&packets), &mut out), Ok(960));
    ms.reset();
    out.clear();
    assert_eq!(ms.decode(None, &mut out), Ok(0));
    assert_eq!(ms.final_ranges(), vec![0, 0]);
}

#[test]
fn the_gain_reaches_every_stream() {
    let m = mapping(1, 2, 0, &[0, 1]);
    let packets = vec![packet(31, false, 1, 60), packet(31, false, 2, 60)];
    let mut plain = MultistreamDecoder::new(&m, 2).unwrap();
    let mut gained = MultistreamDecoder::new(&m, 2).unwrap();
    gained.set_gain(-512);
    let (mut a, mut b) = (vec![], vec![]);
    plain.decode(Some(&join(&packets)), &mut a).unwrap();
    gained.decode(Some(&join(&packets)), &mut b).unwrap();
    let mut single = Decoder::new(1).unwrap();
    single.set_gain(-512);
    let mut one = vec![];
    single.decode(Some(&packets[0]), &mut one).unwrap();
    let left: Vec<f32> = b.iter().step_by(2).copied().collect();
    assert_eq!(left, one);
    assert_ne!(a, b);
}

#[test]
fn hostile_packets_are_refused_and_change_nothing() {
    let m = mapping(1, 2, 1, &[0, 1, 2]);
    let good = vec![packet(1, true, 1, 50), packet(1, false, 2, 50)];
    let mut bad: Vec<Vec<u8>> = vec![
        vec![],
        vec![8],
        // One stream missing: the first, self-delimited, is all there is.
        self_delimited(&good[0]),
        // The streams' durations differ.
        join(&[good[0].clone(), packet(0, false, 3, 30)]),
        // The last stream's packet is invalid (a code 1 with an odd payload).
        join(&[good[0].clone(), vec![(1 << 3) | 1, 1, 2, 3]]),
    ];
    // Every cut of a valid packet.
    let whole = join(&good);
    for cut in 1..self_delimited(&good[0]).len() + 1 {
        bad.push(whole[..cut].to_vec());
    }
    let mut ms = MultistreamDecoder::new(&m, 3).unwrap();
    let mut reference = MultistreamDecoder::new(&m, 3).unwrap();
    let mut out = vec![1.0];
    for p in &bad {
        assert!(
            matches!(ms.decode(Some(p), &mut out), Err(Error::InvalidPacket(_))),
            "{p:?}"
        );
        assert_eq!(out, vec![1.0]);
    }
    // Nothing changed: the next good packet decodes as on a fresh decoder.
    let (mut a, mut b) = (vec![], vec![]);
    ms.decode(Some(&whole), &mut a).unwrap();
    reference.decode(Some(&whole), &mut b).unwrap();
    assert_eq!(a, b);
}

#[test]
fn every_mapping_is_checked_at_construction() {
    let ok = |m: ChannelMapping, ch| MultistreamDecoder::new(&m, ch).is_ok();
    assert!(ok(mapping(1, 1, 1, &[0, 1]), 2));
    assert!(ok(mapping(255, 3, 0, &[0, 1, 2, 255]), 4));
    // The table names a channel not decoded.
    assert!(!ok(mapping(1, 1, 1, &[0, 2]), 2));
    // More coupled streams than streams, none at all, over 255 decoded channels.
    assert!(!ok(mapping(1, 1, 2, &[0, 1]), 2));
    assert!(!ok(mapping(1, 0, 0, &[]), 0));
    assert!(!ok(mapping(255, 255, 1, &[0]), 1));
    // The table and the channel count disagree, or there are no channels.
    assert!(!ok(mapping(1, 1, 1, &[0, 1]), 3));
    assert!(!ok(mapping(1, 1, 0, &[]), 0));
    // Ambisonics is refused by name.
    for family in [2, 3] {
        assert_eq!(
            MultistreamDecoder::new(&mapping(family, 1, 0, &[0]), 1).unwrap_err(),
            Error::Unsupported("ambisonic channel mapping (families 2 and 3) is not supported")
        );
    }
}
