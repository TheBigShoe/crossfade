use super::*;

#[test]
fn three_headers_pack_as_matroska_codec_private_stores_them() {
    let (a, b, c) = (vec![1u8; 30], vec![3u8; 15], vec![5u8; 4]);
    let packed = pack_headers([&a, &b, &c]);
    let mut want = vec![2, 30, 15];
    want.extend(&a);
    want.extend(&b);
    want.extend(&c);
    assert_eq!(packed, want);
    assert_eq!(unpack_headers(&packed), Ok([&a[..], &b[..], &c[..]]));
}

#[test]
fn lengths_of_255_and_more_are_laced_with_255s() {
    for (la, lb) in [(0, 0), (254, 255), (255, 256), (510, 1), (70_000, 300)] {
        let (a, b, c) = (vec![7u8; la], vec![8u8; lb], vec![9u8; 3]);
        let packed = pack_headers([&a, &b, &c]);
        let lacing = la / 255 + 1 + lb / 255 + 1;
        assert_eq!(packed.len(), 1 + lacing + la + lb + 3, "{la} {lb}");
        assert_eq!(unpack_headers(&packed), Ok([&a[..], &b[..], &c[..]]), "{la} {lb}");
    }
    assert_eq!(&pack_headers([&[0; 255], &[], &[]])[..4], &[2, 255, 0, 0]);
}

#[test]
fn bad_extradata_is_refused_not_trusted() {
    let refused = |b: &[u8]| matches!(unpack_headers(b), Err(Error::Malformed(_)));
    assert!(refused(&[]));
    assert!(refused(&[1, 0, 0]));
    assert!(refused(&[2]));
    assert!(refused(&[2, 255, 255]));
    assert!(refused(&[2, 3, 2, 1, 1, 1, 1]));
    assert!(refused(&[2, 255, 255, 255, 255, 255, 255, 255, 1]));
    assert!(!refused(&[2, 3, 2, 1, 1, 1, 1, 1]));
}
