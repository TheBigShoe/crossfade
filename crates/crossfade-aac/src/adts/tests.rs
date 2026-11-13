use super::*;
use crate::Error;

/// Packs the fields into a header, CRC bytes included when `protection_absent` is false.
fn pack(h: &Header) -> Vec<u8> {
    let mut v: u64 = 0xFFF;
    let mut push = |bits: u32, value: u64| v = v << bits | value;
    push(1, u64::from(h.mpeg2));
    push(2, 0);
    push(1, u64::from(h.protection_absent));
    push(2, u64::from(h.profile));
    push(4, u64::from(h.sampling_index));
    push(1, 0);
    push(3, u64::from(h.channel_configuration));
    push(4, 0);
    push(13, u64::from(h.frame_length));
    push(11, u64::from(h.buffer_fullness));
    push(2, u64::from(h.raw_data_blocks - 1));
    let mut out = v.to_be_bytes()[1..].to_vec();
    if !h.protection_absent {
        out.extend([0xAB, 0xCD]);
    }
    out
}

fn lc_stereo_44k() -> Header {
    Header {
        mpeg2: false,
        protection_absent: true,
        profile: 1,
        sampling_index: 4,
        channel_configuration: 2,
        frame_length: 371,
        buffer_fullness: 0x7FF,
        raw_data_blocks: 1,
    }
}

#[test]
fn a_typical_header_parses() {
    // MPEG-4 AAC LC, 44.1 kHz stereo, a 371-byte frame, no CRC.
    let h = Header::parse(&[0xFF, 0xF1, 0x50, 0x80, 0x2E, 0x7F, 0xFC]).unwrap();
    assert_eq!(
        h,
        Header {
            frame_length: 0x173,
            ..lc_stereo_44k()
        }
    );
    assert_eq!(h.header_len(), 7);
    assert_eq!(h.object_type(), 2);
    assert_eq!(h.sample_rate(), 44100);
    assert_eq!(h.payload_len(), 0x173 - 7);
}

#[test]
fn every_field_round_trips() {
    for mpeg2 in [false, true] {
        for protection_absent in [false, true] {
            for profile in 0..4u8 {
                if mpeg2 && profile == 3 {
                    continue;
                }
                for sampling_index in 0..13u8 {
                    for channel_configuration in 0..8u8 {
                        for raw_data_blocks in 1..=4u8 {
                            let h = Header {
                                mpeg2,
                                protection_absent,
                                profile,
                                sampling_index,
                                channel_configuration,
                                frame_length: 8191,
                                buffer_fullness: 0x123,
                                raw_data_blocks,
                            };
                            assert_eq!(Header::parse(&pack(&h)), Ok(h), "{h:?}");
                            let len = if protection_absent { 7 } else { 9 };
                            assert_eq!(h.header_len(), len);
                            assert_eq!(h.sample_rate(), SAMPLE_RATES[sampling_index as usize]);
                        }
                    }
                }
            }
        }
    }
}

fn malformed_text(bytes: &[u8]) -> String {
    match Header::parse(bytes) {
        Err(Error::Malformed(what)) => what,
        other => panic!("expected Malformed, got {other:?}"),
    }
}

#[test]
fn broken_headers_are_malformed_and_say_why() {
    let good = pack(&lc_stereo_44k());
    let with = |f: &dyn Fn(&mut Vec<u8>)| {
        let mut b = good.clone();
        f(&mut b);
        b
    };
    for (bytes, why) in [
        (with(&|b| b[0] = 0xFE), "adts: no sync word"),
        (with(&|b| b[1] &= 0xEF), "adts: no sync word"),
        (with(&|b| b[1] |= 0x02), "adts: layer must be 0"),
        (
            with(&|b| b[2] = b[2] & 0xC3 | 13 << 2),
            "adts: reserved sampling index 13",
        ),
        (
            with(&|b| b[2] = b[2] & 0xC3 | 15 << 2),
            "adts: reserved sampling index 15",
        ),
        (
            with(&|b| {
                b[1] |= 0x08;
                b[2] |= 0xC0;
            }),
            "adts: MPEG-2 profile 3 is reserved",
        ),
    ] {
        assert_eq!(
            malformed_text(&bytes),
            format!("aac: {why}"),
            "{bytes:02X?}"
        );
    }
    let short = Header {
        frame_length: 6,
        ..lc_stereo_44k()
    };
    assert_eq!(
        malformed_text(&pack(&short)),
        "aac: adts: frame length 6 is shorter than its 7-byte header"
    );
    let crc = Header {
        frame_length: 8,
        protection_absent: false,
        ..lc_stereo_44k()
    };
    assert_eq!(
        malformed_text(&pack(&crc)),
        "aac: adts: frame length 8 is shorter than its 9-byte header"
    );
}

#[test]
fn too_few_bytes_are_an_unexpected_end() {
    let good = pack(&lc_stereo_44k());
    for n in 0..7 {
        assert_eq!(Header::parse(&good[..n]), Err(Error::UnexpectedEnd), "{n}");
    }
    let crc = pack(&Header {
        protection_absent: false,
        ..lc_stereo_44k()
    });
    assert_eq!(Header::parse(&crc[..8]), Err(Error::UnexpectedEnd));
}

#[test]
fn the_config_it_stands_for_is_two_bytes_of_audio_specific_config() {
    // object type 2, index 4, two channels: 00010 0100 0010 000
    assert_eq!(lc_stereo_44k().audio_specific_config(), [0x12, 0x10]);
    let mono_8k = Header {
        sampling_index: 11,
        channel_configuration: 1,
        ..lc_stereo_44k()
    };
    assert_eq!(mono_8k.audio_specific_config(), [0x15, 0x88]);
}

#[test]
fn two_headers_of_one_stream_agree_on_the_fixed_fields_only() {
    let a = lc_stereo_44k();
    let b = Header {
        frame_length: 9,
        buffer_fullness: 3,
        protection_absent: false,
        ..a
    };
    assert!(a.same_stream(&b));
    for c in [
        Header { mpeg2: true, ..a },
        Header { profile: 0, ..a },
        Header {
            sampling_index: 3,
            ..a
        },
        Header {
            channel_configuration: 1,
            ..a
        },
    ] {
        assert!(!a.same_stream(&c), "{c:?}");
    }
}

/// A protected frame of `blocks` (when there are several, each followed by a 2-byte CRC),
/// with its position table.
fn protected_frame(blocks: &[&[u8]]) -> Vec<u8> {
    let n = blocks.len();
    let crc = if n > 1 { 2 } else { 0 };
    let table = 2 * (n - 1) + 2;
    let body: usize = blocks.iter().map(|b| b.len() + crc).sum();
    let h = Header {
        protection_absent: false,
        raw_data_blocks: n as u8,
        frame_length: (7 + table + body) as u16,
        ..lc_stereo_44k()
    };
    let mut f = pack(&h)[..7].to_vec();
    let mut at = 0;
    for b in &blocks[..n - 1] {
        at += b.len() + 2;
        f.extend((at as u16).to_be_bytes());
    }
    f.extend([0xC0, 0xC0]); // header CRC
    for b in blocks {
        f.extend(*b);
        f.extend(&[0xCC, 0xCC][..crc]);
    }
    f
}

#[test]
fn a_frame_of_one_block_is_its_payload() {
    let mut f = pack(&Header {
        frame_length: 12,
        ..lc_stereo_44k()
    });
    f.extend([1, 2, 3, 4, 5]);
    let h = Header::parse(&f).unwrap();
    let blocks: Vec<_> = h.raw_data_blocks(&f).unwrap().collect();
    assert_eq!(
        blocks,
        [RawDataBlock {
            data: &[1, 2, 3, 4, 5],
            blocks: 1
        }]
    );
    let crc = protected_frame(&[&[9, 9, 9]]);
    let h = Header::parse(&crc).unwrap();
    let blocks: Vec<_> = h.raw_data_blocks(&crc).unwrap().map(|b| b.data).collect();
    assert_eq!(blocks, [&[9, 9, 9][..]]);
}

#[test]
fn a_protected_frame_of_several_blocks_splits_at_its_positions() {
    let parts: [&[u8]; 4] = [&[1; 5], &[2; 3], &[3; 7], &[4; 1]];
    for n in 2..=4 {
        let f = protected_frame(&parts[..n]);
        let h = Header::parse(&f).unwrap();
        assert_eq!(h.raw_data_blocks, n as u8);
        let blocks: Vec<_> = h.raw_data_blocks(&f).unwrap().collect();
        let want: Vec<_> = parts[..n]
            .iter()
            .map(|&data| RawDataBlock { data, blocks: 1 })
            .collect();
        assert_eq!(blocks, want);
    }
}

#[test]
fn positions_that_do_not_fit_the_frame_are_malformed() {
    let mut f = protected_frame(&[&[1; 5], &[2; 3]]);
    f[8] = 200;
    let h = Header::parse(&f).unwrap();
    assert_eq!(
        h.raw_data_blocks(&f).err(),
        Some(Error::Malformed(
            "aac: adts: raw data block positions do not fit the frame".into()
        ))
    );
    let mut f = protected_frame(&[&[1; 5], &[2; 3]]);
    f[8] = 1; // block 0 shorter than its CRC
    let h = Header::parse(&f).unwrap();
    assert!(h.raw_data_blocks(&f).is_err());
    // The frame given must be as long as the header says.
    let f = protected_frame(&[&[1; 5], &[2; 3]]);
    let h = Header::parse(&f).unwrap();
    assert_eq!(
        h.raw_data_blocks(&f[..f.len() - 1]).err(),
        Some(Error::UnexpectedEnd)
    );
}

#[test]
fn several_blocks_without_a_crc_cannot_be_split_and_come_as_one() {
    let h = Header {
        raw_data_blocks: 3,
        frame_length: 20,
        ..lc_stereo_44k()
    };
    let mut f = pack(&h);
    f.resize(20, 7);
    let blocks: Vec<_> = h.raw_data_blocks(&f).unwrap().collect();
    assert_eq!(
        blocks,
        [RawDataBlock {
            data: &[7; 13],
            blocks: 3
        }]
    );
}
