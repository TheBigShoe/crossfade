#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn hex(d: [u8; 16]) -> String {
        d.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn md5(data: &[u8]) -> String {
        let mut m = Md5::new();
        m.update(data);
        hex(m.finalize())
    }

    // RFC 1321 appendix A.5.
    #[test]
    fn rfc_1321_test_suite() {
        let cases: [(&[u8], &str); 7] = [
            (b"", "d41d8cd98f00b204e9800998ecf8427e"),
            (b"a", "0cc175b9c0f1b6a831c399e269772661"),
            (b"abc", "900150983cd24fb0d6963f7d28e17f72"),
            (b"message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            (
                b"abcdefghijklmnopqrstuvwxyz",
                "c3fcd3d76192e4007dfb496cca67e13b",
            ),
            (
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
                "d174ab98d277d9f5a5611c2c9f419d9f",
            ),
            (
                b"12345678901234567890123456789012345678901234567890123456789012345678901234567890",
                "57edf4a22be3c955ac49da2e2107b67a",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(md5(input), want, "{}", String::from_utf8_lossy(input));
        }
    }

    #[test]
    fn padding_boundaries() {
        // 55, 56 and 64 bytes straddle the length-field and block edges.
        assert_eq!(md5(&[b'a'; 55]), "ef1772b6dff9a122358552954ad0df65");
        assert_eq!(md5(&[b'a'; 56]), "3b0c8ac703f828b04c6c197006d17218");
        assert_eq!(md5(&[b'a'; 64]), "014842d480b571495a4a0363793f7367");
    }

    proptest! {
        #[test]
        fn splitting_the_input_changes_nothing(
            data in prop::collection::vec(any::<u8>(), 0..300),
            cuts in prop::collection::vec(any::<usize>(), 0..6),
        ) {
            let mut points: Vec<usize> = cuts.iter().map(|c| c % (data.len() + 1)).collect();
            points.sort_unstable();
            let mut m = Md5::new();
            let mut at = 0;
            for p in points {
                m.update(&data[at..p]);
                at = p;
            }
            m.update(&data[at..]);
            let mut whole = Md5::new();
            whole.update(&data);
            prop_assert_eq!(m.finalize(), whole.finalize());
        }
    }
}
