//! The Ogg page checksum (RFC 3533 section 6): CRC-32, polynomial 0x04C11DB7, initial value
//! 0, no reflection, no final XOR.

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// The slow definition the table must agree with.
    fn bitwise(mut crc: u32, data: &[u8]) -> u32 {
        for &b in data {
            crc ^= u32::from(b) << 24;
            for _ in 0..8 {
                crc = if crc & 0x8000_0000 != 0 {
                    (crc << 1) ^ 0x04C1_1DB7
                } else {
                    crc << 1
                };
            }
        }
        crc
    }

    #[test]
    fn the_first_page_of_a_real_file_checks_out() {
        let file = include_bytes!("../../tests/fixtures/ogg/stereo16.oga");
        // 27-byte header, one lacing value of 51, then the 51-byte identification packet.
        let mut page = file[..27 + 1 + 51].to_vec();
        let stored = u32::from_le_bytes(page[22..26].try_into().unwrap());
        assert_eq!(stored, 0x12CB_D32B);
        page[22..26].fill(0);
        assert_eq!(crc32(&page), stored);
    }

    #[test]
    fn empty_input_is_the_initial_value() {
        assert_eq!(crc32(&[]), 0);
        assert_eq!(crc32_update(0x1234_5678, &[]), 0x1234_5678);
    }

    proptest! {
        #[test]
        fn the_table_matches_the_bitwise_definition(data in proptest::collection::vec(any::<u8>(), 0..300)) {
            prop_assert_eq!(crc32(&data), bitwise(0, &data));
        }

        #[test]
        fn incremental_equals_whole(data in proptest::collection::vec(any::<u8>(), 0..300), cut in 0usize..300) {
            let cut = cut.min(data.len());
            let (a, b) = data.split_at(cut);
            prop_assert_eq!(crc32_update(crc32(a), b), crc32(&data));
        }
    }
}
