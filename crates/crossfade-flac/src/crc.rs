//! The two checksums of FLAC frames: CRC-8 over the header and CRC-16 over the whole frame
//! (RFC 9639 sections 9.1.8 and 9.3). Both start at 0 and run MSB first.

const fn table8() -> [u8; 256] {
    let mut t = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u8;
        let mut bit = 0;
        while bit < 8 {
            c = if c & 0x80 != 0 {
                (c << 1) ^ 0x07
            } else {
                c << 1
            };
            bit += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

const fn table16() -> [u16; 256] {
    let mut t = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = (i as u16) << 8;
        let mut bit = 0;
        while bit < 8 {
            c = if c & 0x8000 != 0 {
                (c << 1) ^ 0x8005
            } else {
                c << 1
            };
            bit += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

/// `t[k][v]` is the CRC-16 of the byte `v` followed by `k` zero bytes, so eight bytes fold into
/// one step of eight independent lookups.
const fn sliced16() -> [[u16; 256]; 8] {
    let base = table16();
    let mut t = [[0u16; 256]; 8];
    t[0] = base;
    let mut k = 1;
    while k < 8 {
        let mut i = 0;
        while i < 256 {
            let prev = t[k - 1][i];
            t[k][i] = (prev << 8) ^ base[(prev >> 8) as usize];
            i += 1;
        }
        k += 1;
    }
    t
}

static TABLE8: [u8; 256] = table8();
static SLICED16: [[u16; 256]; 8] = sliced16();

/// x^8 + x^2 + x + 1.
pub fn crc8(data: &[u8]) -> u8 {
    crc8_update(0, data)
}

/// Continues a CRC-8 from `crc` (the value over the bytes so far).
pub fn crc8_update(crc: u8, data: &[u8]) -> u8 {
    data.iter().fold(crc, |c, &b| TABLE8[usize::from(c ^ b)])
}

/// x^16 + x^15 + x^2 + 1.
pub fn crc16(data: &[u8]) -> u16 {
    crc16_update(0, data)
}

pub fn crc16_update(mut crc: u16, data: &[u8]) -> u16 {
    let t = &SLICED16;
    let (chunks, rest) = data.as_chunks::<8>();
    for c in chunks {
        let v = crc ^ u16::from_be_bytes([c[0], c[1]]);
        crc = t[7][usize::from(v >> 8)]
            ^ t[6][usize::from(v & 0xff)]
            ^ t[5][usize::from(c[2])]
            ^ t[4][usize::from(c[3])]
            ^ t[3][usize::from(c[4])]
            ^ t[2][usize::from(c[5])]
            ^ t[1][usize::from(c[6])]
            ^ t[0][usize::from(c[7])];
    }
    bytewise16(crc, rest)
}

fn bytewise16(crc: u16, data: &[u8]) -> u16 {
    data.iter().fold(crc, |c, &b| {
        (c << 8) ^ SLICED16[0][usize::from((c >> 8) as u8 ^ b)]
    })
}

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
        let frame = [0xFF, 0xF8, 0xCA, 0x0C, 0x00, 0x7C, 0x01, 0x02, 0x03];
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
        fn sliced_equals_bytewise(
            data in prop::collection::vec(any::<u8>(), 0..300),
            init in any::<u16>(),
        ) {
            prop_assert_eq!(crc16_update(init, &data), bytewise16(init, &data));
        }

        #[test]
        fn incremental_equals_whole(data in prop::collection::vec(any::<u8>(), 0..200), cut in any::<usize>()) {
            let at = cut % (data.len() + 1);
            prop_assert_eq!(crc8_update(crc8(&data[..at]), &data[at..]), crc8(&data));
            prop_assert_eq!(crc16_update(crc16(&data[..at]), &data[at..]), crc16(&data));
        }
    }
}
