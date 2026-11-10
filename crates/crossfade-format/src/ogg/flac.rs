//! The Ogg FLAC mapping (RFC 9639 section 10.1).

#[cfg(test)]
mod tests {
    use super::*;
    use crossfade_core::{Error, Limits, Tags};

    const FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/ogg/stereo16.oga");

    fn fixture_id_packet() -> &'static [u8] {
        &FIXTURE[28..28 + 51]
    }

    fn id_packet(major: u8, minor: u8, count: u16, block_type: u8, len: u32) -> Vec<u8> {
        let mut p = b"\x7fFLAC".to_vec();
        p.extend([major, minor]);
        p.extend(count.to_be_bytes());
        p.extend(b"fLaC");
        p.push(block_type);
        p.extend(&len.to_be_bytes()[1..]);
        p.extend(&fixture_id_packet()[17..51]);
        p
    }

    fn malformed<T: std::fmt::Debug>(r: Result<T, Error>) -> String {
        match r {
            Err(Error::Malformed { what }) => what,
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    #[test]
    fn the_identification_packet_of_a_real_file() {
        let h = parse_first_packet(fixture_id_packet()).unwrap();
        assert_eq!(h.declared_headers, 1);
        assert_eq!(h.block.sample_rate, 44100);
        assert_eq!((h.block.channels, h.block.bits_per_sample), (2, 16));
        assert_eq!(h.block.total_samples, 4410);
        assert_eq!(h.streaminfo, fixture_id_packet()[17..]);
    }

    #[test]
    fn a_new_minor_version_is_fine_but_another_major_is_not() {
        assert!(parse_first_packet(&id_packet(1, 9, 0, 0, 34)).is_ok());
        let err = parse_first_packet(&id_packet(2, 0, 0, 0, 34)).map(|_| ()).unwrap_err();
        assert!(matches!(&err, Error::Unsupported { what } if what.contains("2.0")), "{err:?}");
    }

    #[test]
    fn a_short_or_mislabelled_packet_is_malformed() {
        let good = id_packet(1, 0, 1, 0, 34);
        assert!(malformed(parse_first_packet(&good[..50]).map(|_| ())).contains("short"));
        let mut bad_magic = good.clone();
        bad_magic[0] = 0x7e;
        assert!(parse_first_packet(&bad_magic).is_err());
        let mut bad_flac = good.clone();
        bad_flac[9] = b'x';
        assert!(malformed(parse_first_packet(&bad_flac).map(|_| ())).contains("fLaC"));
        assert!(malformed(parse_first_packet(&id_packet(1, 0, 1, 1, 34)).map(|_| ())).contains("STREAMINFO"));
        assert!(malformed(parse_first_packet(&id_packet(1, 0, 1, 0, 33)).map(|_| ())).contains("STREAMINFO"));
        let mut zero_rate = good;
        zero_rate[27..30].fill(0);
        assert!(parse_first_packet(&zero_rate).is_err());
    }

    #[test]
    fn frames_start_with_a_sync_code() {
        assert!(starts_frame(&[0xFF, 0xF8, 0x00]));
        assert!(starts_frame(&[0xFF, 0xF9]));
        for p in [&[][..], &[0xFF], &[0xFF, 0xFA], &[0xFE, 0xF8], &[0x84, 0, 0, 0]] {
            assert!(!starts_frame(p), "{p:?}");
        }
    }

    fn block(kind: u8, body: &[u8]) -> Vec<u8> {
        let mut p = vec![kind];
        p.extend(&(body.len() as u32).to_be_bytes()[1..]);
        p.extend(body);
        p
    }

    fn comment_body() -> Vec<u8> {
        let mut tags = Tags::new();
        tags.push("TITLE", "Song");
        let mut body = Vec::new();
        crate::vorbis_comment::write("v", &tags, &mut body);
        body
    }

    #[test]
    fn a_vorbis_comment_block_gives_tags_and_the_last_flag_is_ignored() {
        for kind in [4, 0x84] {
            let t = comment_tags(&block(kind, &comment_body()), &Limits::default()).unwrap().unwrap();
            assert_eq!(t.iter().map(|t| (&*t.key, &*t.value)).collect::<Vec<_>>(), [("TITLE", "Song")]);
        }
    }

    #[test]
    fn other_blocks_and_unparsable_comments_give_nothing() {
        let limits = Limits::default();
        assert_eq!(comment_tags(&block(1, &[0; 20]), &limits), Ok(None));
        assert_eq!(comment_tags(&block(3, &[]), &limits), Ok(None));
        assert_eq!(comment_tags(&block(4, &[1, 2, 3]), &limits), Ok(None));
    }

    #[test]
    fn a_block_length_that_overruns_the_packet_is_malformed() {
        let mut p = block(1, &[0; 10]);
        p.truncate(9);
        assert!(malformed(comment_tags(&p, &Limits::default())).contains("block"));
        assert!(malformed(comment_tags(&[4, 0], &Limits::default())).contains("block"));
    }
}
