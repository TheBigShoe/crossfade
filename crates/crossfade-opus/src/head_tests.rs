use super::*;
use proptest::prelude::*;

fn head(channels: u8, family: u8, table: &[u8]) -> Vec<u8> {
    let mut h = b"OpusHead".to_vec();
    h.push(1);
    h.push(channels);
    h.extend(312u16.to_le_bytes());
    h.extend(44_100u32.to_le_bytes());
    h.extend((-256i16).to_le_bytes());
    h.push(family);
    h.extend_from_slice(table);
    h
}

fn bad(r: Result<OpusHead>) -> bool {
    matches!(r, Err(Error::InvalidHeader(_)))
}

#[test]
fn family_0_mono_and_stereo() {
    let h = OpusHead::parse(&head(2, 0, &[])).unwrap();
    assert_eq!(h.version, 1);
    assert_eq!(h.channels, 2);
    assert_eq!(h.pre_skip, 312);
    assert_eq!(h.input_sample_rate, 44_100);
    assert_eq!(h.output_gain, -256);
    assert_eq!(h.output_gain_db(), -1.0);
    assert_eq!(h.mapping.family, 0);
    assert_eq!((h.mapping.streams, h.mapping.coupled), (1, 1));
    assert_eq!(h.mapping.table, vec![0, 1]);
    let h = OpusHead::parse(&head(1, 0, &[])).unwrap();
    assert_eq!((h.mapping.streams, h.mapping.coupled), (1, 0));
    assert_eq!(h.mapping.table, vec![0]);
    // Family 0 allows one or two channels, and never a table.
    assert!(bad(OpusHead::parse(&head(3, 0, &[]))));
    assert!(bad(OpusHead::parse(&head(0, 0, &[]))));
}

#[test]
fn family_1_carries_its_table() {
    // 5.1: four streams, two coupled (FL/FR and RL/RR... as an encoder pairs them).
    let table = [4, 2, 0, 4, 1, 2, 3, 5];
    let h = OpusHead::parse(&head(6, 1, &table)).unwrap();
    assert_eq!(h.mapping.family, 1);
    assert_eq!((h.mapping.streams, h.mapping.coupled), (4, 2));
    assert_eq!(h.mapping.table, vec![0, 4, 1, 2, 3, 5]);
    // At most eight channels.
    let mut nine = vec![5, 4];
    nine.extend(0..9);
    assert!(bad(OpusHead::parse(&head(9, 1, &nine))));
    // 255 stands for silence.
    assert!(OpusHead::parse(&head(2, 1, &[1, 0, 0, 255])).is_ok());
}

#[test]
fn family_255_and_reserved_families_take_any_count() {
    let mut table = vec![255, 0];
    table.extend((0..255).map(|i| i as u8));
    assert!(OpusHead::parse(&head(255, 255, &table)).is_ok());
    // Reserved families (2..=254) parse as 255 does (RFC 7845 5.1.1.4).
    let h = OpusHead::parse(&head(2, 7, &[2, 0, 0, 1])).unwrap();
    assert_eq!(h.mapping.family, 7);
}

#[test]
fn tables_are_checked() {
    // No streams.
    assert!(bad(OpusHead::parse(&head(1, 1, &[0, 0, 0]))));
    // More coupled than streams.
    assert!(bad(OpusHead::parse(&head(1, 1, &[1, 2, 0]))));
    // N + M over 255.
    let mut t = vec![255, 1];
    t.extend([0; 2]);
    assert!(bad(OpusHead::parse(&head(2, 255, &t))));
    // An index past N + M (and not 255).
    assert!(bad(OpusHead::parse(&head(2, 1, &[1, 1, 0, 2]))));
    // The table cut short.
    assert!(bad(OpusHead::parse(&head(2, 1, &[1, 1, 0]))));
    assert!(bad(OpusHead::parse(&head(2, 1, &[1]))));
}

#[test]
fn the_magic_version_and_length_are_checked() {
    let good = head(1, 0, &[]);
    assert!(bad(OpusHead::parse(&good[..18])));
    let mut wrong = good.clone();
    wrong[4] = b'h';
    assert!(bad(OpusHead::parse(&wrong)));
    // A minor version is compatible, with extra data; a major one is not.
    let mut minor = good.clone();
    minor[8] = 15;
    minor.extend([1, 2, 3]);
    assert_eq!(OpusHead::parse(&minor).unwrap().version, 15);
    let mut major = good;
    major[8] = 16;
    assert!(bad(OpusHead::parse(&major)));
}

#[test]
fn the_output_gain_is_q7_8_decibels() {
    let mut h = OpusHead::parse(&head(1, 0, &[])).unwrap();
    h.output_gain = 0;
    assert_eq!(h.output_gain_linear(), 1.0);
    h.output_gain = 20 * 256;
    assert!((h.output_gain_linear() - 10.0).abs() < 1e-12);
    h.output_gain = -6 * 256;
    assert!((h.output_gain_linear() - 0.501_187_233_627_272_2).abs() < 1e-12);
}

fn tags(vendor: &[u8], comments: &[&[u8]], extra: &[u8]) -> Vec<u8> {
    let mut t = b"OpusTags".to_vec();
    t.extend((vendor.len() as u32).to_le_bytes());
    t.extend_from_slice(vendor);
    t.extend((comments.len() as u32).to_le_bytes());
    for c in comments {
        t.extend((c.len() as u32).to_le_bytes());
        t.extend_from_slice(c);
    }
    t.extend_from_slice(extra);
    t
}

#[test]
fn opus_tags_framing() {
    let t = tags(b"crossfade", &[b"TITLE=a", b"ARTIST=b"], &[1, 9, 9]);
    let parsed = OpusTags::parse(&t).unwrap();
    assert_eq!(parsed.vendor, b"crossfade");
    assert_eq!(parsed.comment_count, 2);
    // The Vorbis-comment block for crossfade-format's parser, without the binary data after.
    assert_eq!(parsed.body, &t[8..t.len() - 3]);
    assert_eq!(parsed.extra, &[1, 9, 9]);
    let t = tags(b"", &[], &[]);
    assert_eq!(OpusTags::parse(&t).unwrap().comment_count, 0);
}

#[test]
fn opus_tags_lengths_are_checked() {
    let bad = |d: &[u8]| matches!(OpusTags::parse(d), Err(Error::InvalidHeader(_)));
    assert!(bad(b"OpusTag"));
    assert!(bad(b"OpusHead\0\0\0\0\0\0\0\0"));
    let good = tags(b"v", &[b"A=1"], &[]);
    for cut in 8..good.len() {
        assert!(bad(&good[..cut]), "cut at {cut}");
    }
    // A vendor length past the end.
    let mut t = good.clone();
    t[8] = 200;
    assert!(bad(&t));
    // More comments than could fit.
    let mut t = tags(b"v", &[], &[]);
    let at = t.len() - 4;
    t[at..].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(bad(&t));
}

proptest! {
    #[test]
    fn any_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..300)) {
        let mut head = b"OpusHead".to_vec();
        head.extend(&bytes);
        if let Ok(h) = OpusHead::parse(&head) {
            prop_assert_eq!(h.mapping.table.len(), usize::from(h.channels));
            let decoded = usize::from(h.mapping.streams) + usize::from(h.mapping.coupled);
            prop_assert!(h.mapping.table.iter().all(|&i| i == 255 || usize::from(i) < decoded));
        }
        let mut t = b"OpusTags".to_vec();
        t.extend(&bytes);
        let _ = OpusTags::parse(&t);
        let _ = OpusHead::parse(&bytes);
    }
}
