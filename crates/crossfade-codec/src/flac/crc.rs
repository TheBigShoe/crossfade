#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // Check values for "123456789" from the CRC catalogue (CRC-8/SMBUS, CRC-16/UMTS), the same
    // polynomials and zero init that RFC 9639 section 9.1.8 and 9.3 define.
    #[test]
    fn catalogue_check_values() {
        assert_eq!(crc8(b"123456789"), 0xF4);
        assert_eq!(crc16(b"123456789"), 0xFEE8);
        assert_eq!(crc8(b""), 0);
        assert_eq!(crc16(b""), 0);
    }

    // Headers laid out per RFC 9639 section 9.1 (sync FFF8 fixed block size, then block size,
    // sample rate, channels, sample size, frame number 0). The CRCs come from a bitwise
    // reference of the section 9.1.8 definition, written independently of the table code.
    #[test]
    fn frame_header_crc8() {
        // 4096 samples, 44.1 kHz, stereo, 16 bit.
        assert_eq!(crc8(&[0xFF, 0xF8, 0xC9, 0x18, 0x00]), 0xC2);
        // 4096 samples, 48 kHz, mono, 24 bit.
        assert_eq!(crc8(&[0xFF, 0xF8, 0xCA, 0x0C, 0x00]), 0x7C);
    }

    #[test]
    fn frame_crc16() {
        let frame = [
            0xFF, 0xF8, 0xCA, 0x0C, 0x00, 0x7C, 0x01, 0x02, 0x03,
        ];
        assert_eq!(crc16(&frame), 0xFABB);
        assert_eq!(crc16(&[0xFF]), 0x0202);
    }

    #[test]
    fn appending_the_crc_gives_zero() {
        let mut m = vec![1u8, 2, 3, 4, 5];
        m.push(crc8(&m));
        assert_eq!(crc8(&m), 0);
        let mut m = vec![9u8, 8, 7];
        m.extend(crc16(&m).to_be_bytes());
        assert_eq!(crc16(&m), 0);
    }

    proptest! {
        #[test]
        fn incremental_equals_whole(data in prop::collection::vec(any::<u8>(), 0..200), cut in any::<usize>()) {
            let at = cut % (data.len() + 1);
            prop_assert_eq!(crc8_update(crc8(&data[..at]), &data[at..]), crc8(&data));
            prop_assert_eq!(crc16_update(crc16(&data[..at]), &data[at..]), crc16(&data));
        }
    }
}
