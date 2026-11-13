//! Table-driven Huffman decoding: one lookup on the next [`PRIMARY_BITS`] bits, and for longer
//! codes one more in a sub-table, built once from the codebooks.

use super::codebooks as cb;
use crate::bits::BitReader;
use std::sync::OnceLock;

const PRIMARY_BITS: u32 = 10;
const SUB: u32 = 1 << 31;

pub(crate) struct Book {
    /// Leaf: `len << 16 | symbol` (len 0 never occurs in a complete code). Sub-table pointer:
    /// `SUB | bits << 24 | offset`.
    table: Vec<u32>,
}

impl Book {
    fn new(codes: &[(u8, u32)]) -> Self {
        let primary = 1usize << PRIMARY_BITS;
        let mut table = vec![0u32; primary];
        for (sym, &(len, code)) in codes.iter().enumerate() {
            let len = u32::from(len);
            if len <= PRIMARY_BITS {
                let first = (code << (PRIMARY_BITS - len)) as usize;
                for e in &mut table[first..first + (1 << (PRIMARY_BITS - len))] {
                    *e = len << 16 | sym as u32;
                }
            }
        }
        for prefix in 0..primary as u32 {
            let longer: Vec<_> = codes
                .iter()
                .enumerate()
                .filter(|&(_, &(len, code))| {
                    u32::from(len) > PRIMARY_BITS
                        && code >> (u32::from(len) - PRIMARY_BITS) == prefix
                })
                .collect();
            let Some(bits) = longer
                .iter()
                .map(|&(_, &(len, _))| u32::from(len) - PRIMARY_BITS)
                .max()
            else {
                continue;
            };
            let offset = table.len() as u32;
            table.resize(table.len() + (1 << bits), 0);
            for (sym, &(len, code)) in longer {
                let rest = u32::from(len) - PRIMARY_BITS;
                let suffix = code & ((1 << rest) - 1);
                let first = (offset + (suffix << (bits - rest))) as usize;
                for e in &mut table[first..first + (1 << (bits - rest))] {
                    *e = u32::from(len) << 16 | sym as u32;
                }
            }
            table[prefix as usize] = SUB | bits << 24 | offset;
        }
        Self { table }
    }

    /// The next symbol. Past the end of the input the reader yields zeros, which decode to
    /// some symbol: the caller notices the overrun.
    #[inline]
    pub(crate) fn decode(&self, r: &mut BitReader<'_>) -> u32 {
        let w = r.peek32();
        let e = self.table[(w >> (32 - PRIMARY_BITS)) as usize];
        let e = if e & SUB == 0 {
            e
        } else {
            let bits = (e >> 24) & 0x7F;
            let idx = (w << PRIMARY_BITS) >> (32 - bits);
            self.table[((e & 0xFF_FFFF) + idx) as usize]
        };
        r.skip((e >> 16) as usize);
        e & 0xFFFF
    }
}

/// How a spectral codebook's symbols stand for coefficients.
#[derive(Clone, Copy)]
pub(crate) struct Shape {
    /// Coefficients per codeword: 4 or 2.
    pub(crate) dim: usize,
    /// Values carry their own sign (books 1, 2, 5, 6); else sign bits follow.
    pub(crate) signed: bool,
}

pub(crate) struct Spectral {
    pub(crate) book: Book,
    pub(crate) shape: Shape,
    /// The coefficients each symbol stands for (unsigned books: magnitudes).
    pub(crate) values: Vec<[i8; 4]>,
}

pub(crate) struct Books {
    pub(crate) scalefactor: Book,
    /// Books 1 to 11 at index 0 to 10.
    pub(crate) spectral: Vec<Spectral>,
}

fn widen<const N: usize>(codes: &[(u8, u16); N]) -> Vec<(u8, u32)> {
    codes.iter().map(|&(l, c)| (l, u32::from(c))).collect()
}

fn spectral(codes: Vec<(u8, u32)>, dim: usize, signed: bool, modulo: i32) -> Spectral {
    let offset = if signed { modulo / 2 } else { 0 };
    let values = (0..codes.len() as i32)
        .map(|i| {
            let mut v = [0i8; 4];
            if dim == 4 {
                v = [i / 27, i / 9 % 3, i / 3 % 3, i % 3].map(|x| (x - offset) as i8);
            } else {
                v[0] = (i / modulo - offset) as i8;
                v[1] = (i % modulo - offset) as i8;
            }
            v
        })
        .collect();
    Spectral {
        book: Book::new(&codes),
        shape: Shape { dim, signed },
        values,
    }
}

pub(crate) fn books() -> &'static Books {
    static BOOKS: OnceLock<Books> = OnceLock::new();
    BOOKS.get_or_init(|| Books {
        scalefactor: Book::new(&cb::SCALEFACTOR),
        spectral: vec![
            spectral(widen(&cb::SPECTRUM_1), 4, true, 3),
            spectral(widen(&cb::SPECTRUM_2), 4, true, 3),
            spectral(widen(&cb::SPECTRUM_3), 4, false, 3),
            spectral(widen(&cb::SPECTRUM_4), 4, false, 3),
            spectral(widen(&cb::SPECTRUM_5), 2, true, 9),
            spectral(widen(&cb::SPECTRUM_6), 2, true, 9),
            spectral(widen(&cb::SPECTRUM_7), 2, false, 8),
            spectral(widen(&cb::SPECTRUM_8), 2, false, 8),
            spectral(widen(&cb::SPECTRUM_9), 2, false, 13),
            spectral(widen(&cb::SPECTRUM_10), 2, false, 13),
            spectral(widen(&cb::SPECTRUM_11), 2, false, 17),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_codes() -> Vec<Vec<(u8, u32)>> {
        let mut v = vec![cb::SCALEFACTOR.to_vec()];
        v.extend([
            widen(&cb::SPECTRUM_1),
            widen(&cb::SPECTRUM_2),
            widen(&cb::SPECTRUM_3),
            widen(&cb::SPECTRUM_4),
            widen(&cb::SPECTRUM_5),
            widen(&cb::SPECTRUM_6),
            widen(&cb::SPECTRUM_7),
            widen(&cb::SPECTRUM_8),
            widen(&cb::SPECTRUM_9),
            widen(&cb::SPECTRUM_10),
            widen(&cb::SPECTRUM_11),
        ]);
        v
    }

    #[test]
    fn every_codebook_is_a_complete_prefix_code() {
        for (i, codes) in all_codes().iter().enumerate() {
            let max = codes.iter().map(|c| c.0).max().unwrap();
            // Kraft equality: the code space is filled exactly.
            let kraft: u64 = codes.iter().map(|&(l, _)| 1u64 << (max - l)).sum();
            assert_eq!(kraft, 1 << max, "book {i}");
            for (a, &(la, ca)) in codes.iter().enumerate() {
                assert!(u32::from(la) == 32 || ca >> la == 0, "book {i} code {a}");
                for &(lb, cb) in &codes[a + 1..] {
                    let l = la.min(lb);
                    assert_ne!(
                        ca >> (la - l),
                        cb >> (lb - l),
                        "book {i}: a prefix of another"
                    );
                }
            }
        }
    }

    #[test]
    fn every_codeword_decodes_to_its_symbol() {
        let books = books();
        let tables: Vec<&Book> = std::iter::once(&books.scalefactor)
            .chain(books.spectral.iter().map(|s| &s.book))
            .collect();
        for (codes, book) in all_codes().iter().zip(tables) {
            for (sym, &(len, code)) in codes.iter().enumerate() {
                // The code, then ones, so a wrong length would show.
                let word = (u64::from(code) << (64 - u32::from(len))) | (u64::MAX >> len);
                let bytes = word.to_be_bytes();
                let mut r = BitReader::new(&bytes);
                assert_eq!(book.decode(&mut r), sym as u32);
                assert_eq!(r.remaining(), 64 - usize::from(len));
            }
        }
    }

    #[test]
    fn symbols_unpack_to_their_coefficients() {
        let b = books();
        assert_eq!(b.spectral[0].values[0], [-1, -1, -1, -1]);
        assert_eq!(b.spectral[0].values[40], [0, 0, 0, 0]);
        assert_eq!(b.spectral[2].values[80], [2, 2, 2, 2]);
        assert_eq!(b.spectral[4].values[40][..2], [0, 0]);
        assert_eq!(b.spectral[4].values[0][..2], [-4, -4]);
        assert_eq!(b.spectral[6].values[63][..2], [7, 7]);
        assert_eq!(b.spectral[8].values[168][..2], [12, 12]);
        assert_eq!(b.spectral[10].values[288][..2], [16, 16]);
        assert_eq!(b.spectral[10].values[16][..2], [0, 16]);
    }
}
