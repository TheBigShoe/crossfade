//! CRC-16 of a whole frame, eight bytes per step. `crate::crc::crc16` goes a byte at a time,
//! a serial chain that costs as much as some of the coding work.

const fn tables() -> [[u16; 256]; 8] {
    let mut t = [[0u16; 256]; 8];
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
        t[0][i] = c;
        i += 1;
    }
    // t[k][b] is the CRC of byte b followed by k zero bytes.
    let mut k = 1;
    while k < 8 {
        let mut i = 0;
        while i < 256 {
            let prev = t[k - 1][i];
            t[k][i] = (prev << 8) ^ t[0][(prev >> 8) as usize];
            i += 1;
        }
        k += 1;
    }
    t
}

static TABLES: [[u16; 256]; 8] = tables();

pub(super) fn crc16(data: &[u8]) -> u16 {
    let t = &TABLES;
    let mut crc = 0u16;
    let (chunks, rest) = data.as_chunks::<8>();
    for c in chunks {
        let hi = u16::from_be_bytes([c[0], c[1]]) ^ crc;
        crc = t[7][usize::from(hi >> 8)]
            ^ t[6][usize::from(hi & 0xFF)]
            ^ t[5][usize::from(c[2])]
            ^ t[4][usize::from(c[3])]
            ^ t[3][usize::from(c[4])]
            ^ t[2][usize::from(c[5])]
            ^ t[1][usize::from(c[6])]
            ^ t[0][usize::from(c[7])];
    }
    for &b in rest {
        crc = (crc << 8) ^ t[0][usize::from((crc >> 8) as u8 ^ b)];
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equals_the_bytewise_crc_at_every_length() {
        let mut s = 0x2545_f491_4f6c_dd1du64;
        let data: Vec<u8> = (0..300)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                (s >> 24) as u8
            })
            .collect();
        for n in 0..data.len() {
            assert_eq!(crc16(&data[..n]), crate::crc::crc16(&data[..n]), "{n}");
        }
        assert_eq!(crc16(b"123456789"), 0xFEE8);
    }
}
