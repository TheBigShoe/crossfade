use super::*;
use crate::Error;

fn ident_bytes(channels: u8, rate: u32, bs: u8) -> Vec<u8> {
    let mut b = vec![1];
    b.extend_from_slice(b"vorbis");
    b.extend_from_slice(&0u32.to_le_bytes());
    b.push(channels);
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&320_000i32.to_le_bytes());
    b.extend_from_slice(&128_000i32.to_le_bytes());
    b.extend_from_slice(&(-1i32).to_le_bytes());
    b.push(bs);
    b.push(1);
    b
}

fn malformed(r: std::result::Result<impl std::fmt::Debug, Error>) -> String {
    match r {
        Err(Error::Malformed(what)) => what,
        other => panic!("expected Malformed, got {other:?}"),
    }
}

#[test]
fn an_identification_header_gives_channels_rate_block_sizes_and_bitrates() {
    let id = Ident::parse(&ident_bytes(2, 44_100, 0xB8)).unwrap();
    assert_eq!(
        id,
        Ident {
            channels: 2,
            sample_rate: 44_100,
            bitrate_maximum: 320_000,
            bitrate_nominal: 128_000,
            bitrate_minimum: -1,
            block_sizes: [256, 2048],
        }
    );
}

#[test]
fn every_block_size_from_64_to_8192_is_taken() {
    for e0 in 6..=13u8 {
        for e1 in e0..=13u8 {
            let id = Ident::parse(&ident_bytes(1, 8000, e0 | (e1 << 4))).unwrap();
            assert_eq!(id.block_sizes, [1 << e0, 1 << e1]);
        }
    }
}

#[test]
fn a_bad_identification_header_is_refused_with_its_reason() {
    let good = ident_bytes(2, 44_100, 0xB8);
    let with = |f: &dyn Fn(&mut Vec<u8>)| {
        let mut b = good.clone();
        f(&mut b);
        malformed(Ident::parse(&b))
    };
    assert!(with(&|b| b.truncate(29)).contains("short"));
    assert!(with(&|b| b[0] = 3).contains("identification"));
    assert!(with(&|b| b[3] = b'X').contains("identification"));
    assert!(with(&|b| b[7] = 1).contains("version"));
    assert!(with(&|b| b[11] = 0).contains("channel"));
    assert!(with(&|b| b[12..16].fill(0)).contains("rate"));
    assert!(with(&|b| b[28] = 0x85).contains("block size"));
    assert!(with(&|b| b[28] = 0xE8).contains("block size"));
    assert!(with(&|b| b[28] = 0x8B).contains("block size"));
    assert!(with(&|b| b[29] = 0).contains("framing"));
}

fn comment_bytes(vendor: &[u8], entries: &[&[u8]]) -> Vec<u8> {
    let mut b = vec![3];
    b.extend_from_slice(b"vorbis");
    b.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    b.extend_from_slice(vendor);
    b.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for e in entries {
        b.extend_from_slice(&(e.len() as u32).to_le_bytes());
        b.extend_from_slice(e);
    }
    b.push(1);
    b
}

#[test]
fn a_comment_header_gives_the_vendor_and_key_value_pairs() {
    let b = comment_bytes(
        b"Xiph.Org libVorbis I 20200704",
        &[b"TITLE=One", b"artist=Two=2", b"junk"],
    );
    let c = Comments::parse(&b, 1 << 20).unwrap();
    assert_eq!(c.vendor, "Xiph.Org libVorbis I 20200704");
    assert_eq!(c.entries, ["TITLE=One", "artist=Two=2", "junk"]);
    let pairs: Vec<_> = c.pairs().collect();
    assert_eq!(pairs, [("TITLE", "One"), ("artist", "Two=2")]);
}

#[test]
fn a_comment_header_without_its_framing_bit_is_still_read() {
    let mut b = comment_bytes(b"v", &[b"A=b"]);
    b.pop();
    assert_eq!(Comments::parse(&b, 100).unwrap().entries, ["A=b"]);
}

#[test]
fn invalid_utf8_in_a_comment_is_replaced_not_refused() {
    let b = comment_bytes(b"v\xFF", &[b"A=\xC3"]);
    let c = Comments::parse(&b, 100).unwrap();
    assert_eq!(c.vendor, "v\u{FFFD}");
    assert_eq!(c.entries, ["A=\u{FFFD}"]);
}

#[test]
fn comment_lengths_are_checked_against_the_packet_before_anything_is_allocated() {
    // A vendor length of 1 GiB in a 20-byte packet.
    let mut b = vec![3];
    b.extend_from_slice(b"vorbis");
    b.extend_from_slice(&(1u32 << 30).to_le_bytes());
    b.extend_from_slice(b"abcdefghi");
    assert!(malformed(Comments::parse(&b, usize::MAX)).contains("vendor"));
    // Four billion comments promised, none there.
    let mut b = comment_bytes(b"v", &[]);
    b.truncate(b.len() - 5);
    b.extend_from_slice(&u32::MAX.to_le_bytes());
    assert!(malformed(Comments::parse(&b, usize::MAX)).contains("comment"));
    // One comment whose length runs past the end.
    let mut b = comment_bytes(b"v", &[b"A=b"]);
    let at = b.len() - 8;
    b[at..at + 4].copy_from_slice(&1000u32.to_le_bytes());
    assert!(malformed(Comments::parse(&b, usize::MAX)).contains("comment"));
}

#[test]
fn comments_past_the_callers_byte_limit_are_refused() {
    let b = comment_bytes(b"vendor", &[b"A=0123456789"]);
    assert!(Comments::parse(&b, 18).is_ok());
    assert_eq!(
        Comments::parse(&b, 17).err(),
        Some(Error::OverBudget {
            requested: 18,
            budget: 17
        })
    );
    assert!(malformed(Comments::parse(&ident_bytes(1, 8000, 0x88), 100)).contains("comment"));
}

#[test]
fn header_kind_tells_the_three_headers_from_audio() {
    assert_eq!(
        header_kind(&ident_bytes(1, 8000, 0x88)),
        Some(HeaderKind::Ident)
    );
    assert_eq!(
        header_kind(&comment_bytes(b"", &[])),
        Some(HeaderKind::Comment)
    );
    assert_eq!(header_kind(b"\x05vorbis\x00"), Some(HeaderKind::Setup));
    assert_eq!(header_kind(b"\x00"), None);
    assert_eq!(header_kind(b"\x05vorbix"), None);
    assert_eq!(header_kind(b"\x07vorbis"), None);
    assert_eq!(header_kind(b""), None);
}
