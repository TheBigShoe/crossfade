//! Table-driven Huffman decoding: one lookup on the next [`PRIMARY_BITS`] bits, and for longer
//! codes one more in a sub-table, built once from the codebooks. A spectral book's entries
//! carry the coefficients themselves and which of them take a sign bit, so one lookup and one
//! peek decode a whole codeword with its signs.

use super::codebooks as cb;
use crate::bits::BitReader;
use std::sync::OnceLock;

const PRIMARY_BITS: u32 = 10;

/// Entries: bits 0-4 the bits a leaf consumes (never 0), 0 for a pointer to a sub-table (bits 5-9 its
/// index bits, bits 10-31 its offset in `sub`). A leaf's other bits are the book's own: see
/// [`Book::decode`] and [`Spectral::decode`].
pub(crate) struct Book {
    primary: Box<[u32; 1 << PRIMARY_BITS]>,
    sub: Vec<u32>,
}

impl Book {
    /// `leaf(symbol, length)` is a leaf's entry; its low five bits must be nonzero (the bits
    /// to consume).
    fn new(codes: &[(u8, u32)], leaf: impl Fn(usize, u32) -> u32) -> Self {
        let mut primary = Box::new([0u32; 1 << PRIMARY_BITS]);
        let mut sub = Vec::new();
        for (sym, &(len, code)) in codes.iter().enumerate() {
            let len = u32::from(len);
            if len <= PRIMARY_BITS {
                let first = (code << (PRIMARY_BITS - len)) as usize;
                for e in &mut primary[first..first + (1 << (PRIMARY_BITS - len))] {
                    *e = leaf(sym, len);
                }
            }
        }
        for prefix in 0..1u32 << PRIMARY_BITS {
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
            let offset = sub.len() as u32;
            sub.resize(sub.len() + (1 << bits), 0);
            for (sym, &(len, code)) in longer {
                let rest = u32::from(len) - PRIMARY_BITS;
                let suffix = code & ((1 << rest) - 1);
                let first = (offset + (suffix << (bits - rest))) as usize;
                for e in &mut sub[first..first + (1 << (bits - rest))] {
                    *e = leaf(sym, u32::from(len));
                }
            }
            primary[prefix as usize] = offset << 10 | bits << 5;
        }
        Self { primary, sub }
    }

    /// The leaf for the codeword at the top of `w`.
    #[inline(always)]
    fn entry(&self, w: u32) -> u32 {
        let e = self.primary[(w >> (32 - PRIMARY_BITS)) as usize];
        if e & 31 != 0 {
            return e;
        }
        let bits = (e >> 5) & 31;
        let idx = (w << PRIMARY_BITS) >> (32 - bits);
        // A complete code gives every pointer a full sub-table; the fallback never happens.
        self.sub
            .get((e >> 10) as usize + idx as usize)
            .copied()
            .unwrap_or(1)
    }

    /// The next symbol of a book built with [`symbol_leaf`]. Past the end of the input the
    /// reader yields zeros, which decode to some symbol: the caller notices the overrun.
    #[inline]
    pub(crate) fn decode(&self, r: &mut BitReader<'_>) -> u32 {
        let e = self.entry(r.peek32());
        r.consume(e & 31);
        e >> 8
    }
}

fn symbol_leaf(sym: usize, len: u32) -> u32 {
    (sym as u32) << 8 | len
}

/// A spectral book (1 to 11).
pub(crate) struct Spectral {
    book: Book,
}

/// Bits per coefficient in a spectral leaf: quads hold -2..=2, pairs -16..=16.
const fn value_bits(dim: usize) -> u32 {
    if dim == 4 { 3 } else { 6 }
}

impl Spectral {
    /// The next codeword's coefficients with their signs applied (an escape stays 16 for
    /// the caller to read). Leaf: bits 0-4 the bits the codeword and its signs take, 5-8
    /// which coefficients take a sign bit, 9-13 the codeword's length, 14-25 the coefficients
    /// (`value_bits` each, two's complement, first lowest).
    #[inline(always)]
    pub(crate) fn decode<const DIM: usize>(&self, r: &mut BitReader<'_>) -> [i32; DIM] {
        let w = r.peek32();
        let e = self.book.entry(w);
        // At most 16 code bits and 4 sign bits, so the peek holds them all.
        let mut signs = w << (e >> 9 & 31);
        let bits = value_bits(DIM);
        let mut q = [0i32; DIM];
        for (i, q) in q.iter_mut().enumerate() {
            let v = ((e << (32 - 14 - bits * (i as u32 + 1))) as i32) >> (32 - bits);
            *q = if e >> (5 + i) & 1 != 0 {
                let neg = signs >> 31 != 0;
                signs <<= 1;
                if neg { -v } else { v }
            } else {
                v
            };
        }
        // The total sits in the low bits, so the next lookup waits on one mask, not a sum.
        r.consume(e & 31);
        q
    }
}

/// The coefficients symbol `i` of a book stands for (unsigned books: magnitudes).
fn symbol_values(i: i32, dim: usize, signed: bool, modulo: i32) -> [i32; 4] {
    let offset = if signed { modulo / 2 } else { 0 };
    let mut v = [0i32; 4];
    if dim == 4 {
        v = [i / 27, i / 9 % 3, i / 3 % 3, i % 3].map(|x| x - offset);
    } else {
        v[0] = i / modulo - offset;
        v[1] = i % modulo - offset;
    }
    v
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
    let bits = value_bits(dim);
    let leaf = |sym: usize, len: u32| {
        let v = symbol_values(sym as i32, dim, signed, modulo);
        let mut e = len << 9;
        let mut count = 0;
        for (i, &x) in v[..dim].iter().enumerate() {
            if !signed && x != 0 {
                e |= 1 << (5 + i);
                count += 1;
            }
            e |= ((x as u32) & ((1 << bits) - 1)) << (14 + bits * i as u32);
        }
        e | (len + count)
    };
    Spectral {
        book: Book::new(&codes, leaf),
    }
}

pub(crate) fn books() -> &'static Books {
    static BOOKS: OnceLock<Books> = OnceLock::new();
    BOOKS.get_or_init(|| Books {
        scalefactor: Book::new(&cb::SCALEFACTOR, symbol_leaf),
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

    /// A spectral book's codes with its shape: (codes, dim, signed, modulo).
    type Shaped = (Vec<(u8, u32)>, usize, bool, i32);

    fn spectral_codes() -> Vec<Shaped> {
        vec![
            (widen(&cb::SPECTRUM_1), 4, true, 3),
            (widen(&cb::SPECTRUM_2), 4, true, 3),
            (widen(&cb::SPECTRUM_3), 4, false, 3),
            (widen(&cb::SPECTRUM_4), 4, false, 3),
            (widen(&cb::SPECTRUM_5), 2, true, 9),
            (widen(&cb::SPECTRUM_6), 2, true, 9),
            (widen(&cb::SPECTRUM_7), 2, false, 8),
            (widen(&cb::SPECTRUM_8), 2, false, 8),
            (widen(&cb::SPECTRUM_9), 2, false, 13),
            (widen(&cb::SPECTRUM_10), 2, false, 13),
            (widen(&cb::SPECTRUM_11), 2, false, 17),
        ]
    }

    fn all_codes() -> Vec<Vec<(u8, u32)>> {
        let mut v = vec![cb::SCALEFACTOR.to_vec()];
        v.extend(spectral_codes().into_iter().map(|c| c.0));
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

    /// The code, then `fill`, so a wrong length would show.
    fn reader_bytes(len: u8, code: u32, fill: u64) -> [u8; 8] {
        ((u64::from(code) << (64 - u32::from(len))) | (fill >> len)).to_be_bytes()
    }

    #[test]
    fn every_scalefactor_codeword_decodes_to_its_symbol() {
        let book = &books().scalefactor;
        for (sym, &(len, code)) in cb::SCALEFACTOR.iter().enumerate() {
            let bytes = reader_bytes(len, code, u64::MAX);
            let mut r = BitReader::new(&bytes);
            assert_eq!(book.decode(&mut r), sym as u32);
            assert_eq!(r.remaining(), 64 - usize::from(len));
        }
    }

    #[test]
    fn every_spectral_codeword_decodes_to_its_coefficients_and_signs() {
        for (b, (codes, dim, signed, modulo)) in spectral_codes().into_iter().enumerate() {
            let book = &books().spectral[b];
            for (sym, &(len, code)) in codes.iter().enumerate() {
                let want = symbol_values(sym as i32, dim, signed, modulo);
                let nonzero = want[..dim].iter().filter(|&&v| v != 0).count();
                // Sign bits 1, 0, 1, 0: the first and third nonzero values negative.
                for (fill, flips) in [
                    (u64::MAX, [true; 4]),
                    (0xAAAA << 48, [true, false, true, false]),
                ] {
                    let bytes = reader_bytes(len, code, fill);
                    let mut r = BitReader::new(&bytes);
                    let got: Vec<i32> = if dim == 4 {
                        book.decode::<4>(&mut r).to_vec()
                    } else {
                        book.decode::<2>(&mut r).to_vec()
                    };
                    let mut k = 0;
                    let want: Vec<i32> = want[..dim]
                        .iter()
                        .map(|&v| {
                            if signed || v == 0 {
                                return v;
                            }
                            k += 1;
                            if flips[k - 1] { -v } else { v }
                        })
                        .collect();
                    assert_eq!(got, want, "book {} symbol {sym}", b + 1);
                    let signs = if signed { 0 } else { nonzero };
                    assert_eq!(r.remaining(), 64 - usize::from(len) - signs);
                }
            }
        }
    }

    #[test]
    fn symbols_unpack_to_their_coefficients() {
        assert_eq!(symbol_values(0, 4, true, 3), [-1, -1, -1, -1]);
        assert_eq!(symbol_values(40, 4, true, 3), [0, 0, 0, 0]);
        assert_eq!(symbol_values(80, 4, false, 3), [2, 2, 2, 2]);
        assert_eq!(symbol_values(40, 2, true, 9)[..2], [0, 0]);
        assert_eq!(symbol_values(0, 2, true, 9)[..2], [-4, -4]);
        assert_eq!(symbol_values(63, 2, false, 8)[..2], [7, 7]);
        assert_eq!(symbol_values(168, 2, false, 13)[..2], [12, 12]);
        assert_eq!(symbol_values(288, 2, false, 17)[..2], [16, 16]);
        assert_eq!(symbol_values(16, 2, false, 17)[..2], [0, 16]);
    }
}
