use super::*;

fn frames(sizes: &[usize]) -> Vec<Vec<u8>> {
    sizes
        .iter()
        .enumerate()
        .map(|(k, &n)| (0..n).map(|i| (i * 7 + k * 31) as u8).collect())
        .collect()
}

fn refs(f: &[Vec<u8>]) -> Vec<&[u8]> {
    f.iter().map(Vec::as_slice).collect()
}

#[test]
fn frames_come_back_from_the_packet_with_the_reference_code() {
    // One frame is code 0, two equal ones 1, two unequal 2, more 3.
    for (sizes, code) in [
        (vec![0], 0),
        (vec![40], 0),
        (vec![1275], 0),
        (vec![30, 30], 1),
        (vec![30, 300], 2),
        (vec![300, 30], 2),
        (vec![50, 50, 50], 3),
        (vec![50, 400, 7], 3),
    ] {
        let f = frames(&sizes);
        let p = assemble(0x7d, &refs(&f), 4000, false, false).expect("fits");
        assert_eq!(p[0] & 3, code, "{sizes:?}");
        assert_eq!(p[0] & 0xFC, 0x7c);
        let parsed = Packet::parse(&p).expect("parses");
        assert_eq!(parsed.frames(), refs(&f).as_slice());
        assert!(parsed.padding().is_empty());
    }
}

#[test]
fn padding_fills_to_the_size_asked() {
    for sizes in [vec![10], vec![20, 20], vec![20, 21], vec![5, 6, 7]] {
        let f = frames(&sizes);
        for maxlen in [60, 300, 1000, 1276] {
            let p = assemble(0x80, &refs(&f), maxlen, false, true).expect("fits");
            assert_eq!(p.len(), maxlen, "{sizes:?} {maxlen}");
            let parsed = Packet::parse(&p).expect("parses");
            assert_eq!(parsed.frames(), refs(&f).as_slice());
        }
    }
    let mut p = vec![0x80, 1, 2, 3];
    pad(&mut p, 4).expect("same size");
    assert_eq!(p, [0x80, 1, 2, 3]);
    pad(&mut p, 5).expect("one more");
    assert_eq!(p, [0x83, 1, 1, 2, 3]);
    pad(&mut p, 20).expect("pads");
    assert_eq!(p.len(), 20);
    assert!(pad(&mut p, 10).is_err());
}

#[test]
fn self_delimited_packets_parse_as_such() {
    for sizes in [
        vec![0],
        vec![300],
        vec![9, 9],
        vec![9, 260],
        vec![3, 4, 500],
    ] {
        let f = frames(&sizes);
        for pad_to in [None, Some(900)] {
            let maxlen = pad_to.unwrap_or(4000);
            let p = assemble(0x04, &refs(&f), maxlen, true, pad_to.is_some()).expect("fits");
            let mut stream = p.clone();
            stream.extend_from_slice(&[0xAB; 5]);
            let parsed = Packet::parse_self_delimited(&stream).expect("parses");
            assert_eq!(parsed.frames(), refs(&f).as_slice(), "{sizes:?}");
            assert_eq!(parsed.byte_len(), p.len());
        }
    }
}

#[test]
fn what_does_not_fit_is_refused() {
    let f = frames(&[100, 100]);
    assert!(assemble(0, &refs(&f), 200, false, false).is_err());
    assert!(assemble(0, &refs(&f), 201, false, false).is_ok());
    assert!(assemble(0, &[], 100, false, false).is_err());
}

#[test]
fn reframing_drops_the_padding_and_keeps_the_frames() {
    let f = frames(&[12, 30]);
    let padded = assemble(0x84, &refs(&f), 500, false, true).expect("fits");
    let plain = reframe(&padded, 4000, false, false).expect("fits");
    assert_eq!(
        plain,
        assemble(0x84, &refs(&f), 4000, false, false).expect("fits")
    );
    let delimited = reframe(&padded, 4000, true, false).expect("fits");
    let parsed = Packet::parse_self_delimited(&delimited).expect("parses");
    assert_eq!(parsed.frames(), refs(&f).as_slice());
}
