#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn le(n: u32) -> [u8; 4] {
        n.to_le_bytes()
    }

    fn hand_built() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(le(6));
        b.extend(b"vendor");
        b.extend(le(3));
        for f in [&b"ARTIST=Someone"[..], b"title=A=B", b"no equals sign"] {
            b.extend(le(f.len() as u32));
            b.extend(f);
        }
        b
    }

    fn parse_ok(body: &[u8]) -> VorbisComment {
        parse(body, &Limits::default()).unwrap()
    }

    #[test]
    fn parses_a_hand_built_block() {
        let c = parse_ok(&hand_built());
        assert_eq!(c.vendor, "vendor");
        // A field without '=' is skipped; the value keeps later '='; keys are as written.
        let all: Vec<_> = c.tags.iter().map(|t| (t.key.as_str(), t.value.as_str())).collect();
        assert_eq!(all, [("ARTIST", "Someone"), ("title", "A=B")]);
        assert_eq!(c.tags.get("TITLE"), Some("A=B"));
    }

    #[test]
    fn invalid_utf8_is_replaced_not_an_error() {
        let mut b = Vec::new();
        b.extend(le(2));
        b.extend([0xFF, b'v']);
        b.extend(le(1));
        let f = [b'K', b'=', 0xC3, 0x28];
        b.extend(le(f.len() as u32));
        b.extend(f);
        let c = parse_ok(&b);
        assert_eq!(c.vendor, "\u{FFFD}v");
        assert_eq!(c.tags.get("K"), Some("\u{FFFD}("));
    }

    #[test]
    fn extra_bytes_after_the_last_field_are_ignored() {
        let mut b = hand_built();
        b.push(1);
        assert_eq!(parse_ok(&b).tags.len(), 2);
    }

    #[test]
    fn truncation_at_every_length_is_malformed() {
        let full = hand_built();
        for n in 0..full.len() {
            assert!(
                matches!(parse(&full[..n], &Limits::default()), Err(Error::Malformed { .. })),
                "{n}"
            );
        }
        assert!(parse(&full, &Limits::default()).is_ok());
    }

    #[test]
    fn huge_declared_lengths_and_counts_fail_without_allocating() {
        let m = |b: Vec<u8>| parse(&b, &Limits::default());
        let mut vendor = le(u32::MAX).to_vec();
        vendor.extend(b"xx");
        assert!(matches!(m(vendor), Err(Error::Malformed { .. })));

        let mut count = le(0).to_vec();
        count.extend(le(u32::MAX));
        assert!(matches!(m(count), Err(Error::Malformed { .. })));

        let mut field = le(0).to_vec();
        field.extend(le(1));
        field.extend(le(u32::MAX));
        field.extend(b"A=b");
        assert!(matches!(m(field), Err(Error::Malformed { .. })));
    }

    #[test]
    fn the_allocation_limit_is_honoured() {
        let tiny = Limits {
            max_alloc_bytes: 4,
            ..Limits::default()
        };
        assert!(matches!(
            parse(&hand_built(), &tiny),
            Err(Error::AllocLimitExceeded { .. })
        ));
    }

    #[test]
    fn writes_the_documented_layout() {
        let mut tags = Tags::new();
        tags.push("A", "b");
        let mut out = Vec::new();
        write("v", &tags, &mut out);
        let mut want = Vec::new();
        want.extend(le(1));
        want.push(b'v');
        want.extend(le(1));
        want.extend(le(3));
        want.extend(b"A=b");
        assert_eq!(out, want);
    }

    proptest! {
        #[test]
        fn write_then_parse_round_trips(
            vendor in ".{0,20}",
            entries in prop::collection::vec(("[^=]{0,12}", ".{0,30}"), 0..8),
        ) {
            let mut tags = Tags::new();
            for (k, v) in &entries {
                tags.push(k.clone(), v.clone());
            }
            let mut out = Vec::new();
            write(&vendor, &tags, &mut out);
            let back = parse(&out, &Limits::default()).unwrap();
            prop_assert_eq!(back.vendor, vendor);
            prop_assert_eq!(back.tags, tags);
        }

        #[test]
        fn arbitrary_bytes_never_panic(body in prop::collection::vec(any::<u8>(), 0..120)) {
            let _ = parse(&body, &Limits::default());
        }
    }
}
