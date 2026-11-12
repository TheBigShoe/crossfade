//! Codebooks (specification section 3): the Huffman code of each book, decoded through a
//! lookup table, and its vector values, unpacked once at setup.
//!
//! The codeword assignment and its checks follow libvorbis's `_make_words` and
//! `vorbis_staticbook_unpack` (Copyright Xiph.Org Foundation, BSD-3-Clause).

use crate::bits::{BitReader, ilog};
use crate::error::{Error, Result, malformed};

/// Counts the bytes a setup allocates against the caller's limit, before each allocation.
#[derive(Debug)]
pub(crate) struct Budget {
    used: usize,
    limit: usize,
}

impl Budget {
    pub(crate) fn new(limit: usize) -> Self {
        Self { used: 0, limit }
    }

    pub(crate) fn used(&self) -> usize {
        self.used
    }

    pub(crate) fn charge(&mut self, bytes: usize) -> Result<()> {
        let requested = self.used.saturating_add(bytes);
        if requested > self.limit {
            return Err(Error::OverBudget {
                requested,
                budget: self.limit,
            });
        }
        self.used = requested;
        Ok(())
    }

    /// `n` copies of `fill`, charged first.
    pub(crate) fn vec<T: Clone>(&mut self, n: usize, fill: T) -> Result<Vec<T>> {
        self.charge(n.saturating_mul(size_of::<T>()))?;
        Ok(vec![fill; n])
    }

    /// An empty vector with room for `n`, charged first.
    pub(crate) fn with_capacity<T>(&mut self, n: usize) -> Result<Vec<T>> {
        self.charge(n.saturating_mul(size_of::<T>()))?;
        Ok(Vec::with_capacity(n))
    }
}

/// Codewords up to this long decode with one table lookup.
const TABLE_BITS: u32 = 10;

#[derive(Debug, Clone)]
pub(crate) struct Codebook {
    dims: usize,
    entries: u32,
    lookup: bool,
    table_bits: u32,
    /// Indexed by the next `table_bits` bits: `symbol << 6 | length`, or 0 for a codeword
    /// longer than the table.
    table: Vec<u32>,
    /// The longer codewords, most significant bit first and left-aligned, ascending.
    long_codes: Vec<u32>,
    /// `symbol << 6 | length` for each of `long_codes`.
    long_info: Vec<u32>,
    /// The entry number of each symbol (the used entries, in order).
    entry_of: Vec<u32>,
    /// `dims` values per symbol when the book has a lookup.
    vq: Vec<f32>,
}

impl Codebook {
    pub(crate) fn parse(r: &mut BitReader<'_>, budget: &mut Budget) -> Result<Self> {
        let eof = || malformed("setup header ends inside a codebook");
        if r.read(24).ok_or_else(eof)? != 0x56_4342 {
            return Err(malformed("codebook sync pattern missing"));
        }
        let dims = r.read(16).ok_or_else(eof)?;
        let entries = r.read(24).ok_or_else(eof)?;
        if dims == 0 {
            return Err(malformed("a codebook of zero dimensions"));
        }
        // libvorbis's bound, which also keeps dims * entries below 2^24.
        if ilog(dims) + ilog(entries) > 24 {
            return Err(malformed(format!(
                "codebook too large: {entries} entries of {dims} dimensions"
            )));
        }
        let n = entries as usize;
        let ordered = r.read_bit().ok_or_else(eof)?;
        let lengths = if ordered {
            let mut lengths = budget.vec(n, 0u8)?;
            let mut len = r.read(5).ok_or_else(eof)? + 1;
            let mut at = 0u32;
            while at < entries {
                let num = r.read(ilog(entries - at)).ok_or_else(eof)?;
                if len > 32 {
                    return Err(malformed("codeword length over 32"));
                }
                if num > entries - at {
                    return Err(malformed(format!(
                        "a run of {num} codeword lengths past the codebook's {entries} entries"
                    )));
                }
                if num > 0 && u64::from(num - 1) >> (len - 1) > 1 {
                    return Err(malformed(format!(
                        "{num} codewords of length {len} cannot exist"
                    )));
                }
                lengths[at as usize..(at + num) as usize].fill(len as u8);
                at += num;
                len += 1;
            }
            lengths
        } else {
            let sparse = r.read_bit().ok_or_else(eof)?;
            let least = n.saturating_mul(if sparse { 1 } else { 5 });
            if least > r.bits_left() {
                return Err(eof());
            }
            let mut lengths = budget.vec(n, 0u8)?;
            for l in &mut lengths {
                if !sparse || r.read_bit().ok_or_else(eof)? {
                    *l = r.read(5).ok_or_else(eof)? as u8 + 1;
                }
            }
            lengths
        };

        let lookup_type = r.read(4).ok_or_else(eof)?;
        let lookup = match lookup_type {
            0 => None,
            1 | 2 => {
                let minimum = float32_unpack(r.read(32).ok_or_else(eof)?);
                let delta = float32_unpack(r.read(32).ok_or_else(eof)?);
                let value_bits = r.read(4).ok_or_else(eof)? + 1;
                let sequence = r.read_bit().ok_or_else(eof)?;
                let values = if lookup_type == 1 {
                    u64::from(lookup1_values(entries, dims as u16))
                } else {
                    u64::from(entries) * u64::from(dims)
                };
                if values.saturating_mul(u64::from(value_bits)) > r.bits_left() as u64 {
                    return Err(malformed(
                        "codebook multiplicands run past the end of the header",
                    ));
                }
                let mut multiplicands = budget.with_capacity(values as usize)?;
                for _ in 0..values {
                    multiplicands.push(r.read(value_bits).ok_or_else(eof)?);
                }
                Some((lookup_type, minimum, delta, sequence, multiplicands))
            }
            t => return Err(malformed(format!("codebook lookup type {t}"))),
        };

        let mut book = Self::build(&lengths, budget)?;
        book.dims = dims as usize;
        book.entries = entries;
        if let Some((kind, minimum, delta, sequence, multiplicands)) = lookup {
            book.lookup = true;
            book.vq = unquantize(
                &book.entry_of,
                book.dims,
                kind,
                minimum,
                delta,
                sequence,
                &multiplicands,
                budget,
            )?;
        }
        Ok(book)
    }

    /// The Huffman decoder of a book with these codeword lengths (0 for an unused entry).
    fn build(lengths: &[u8], budget: &mut Budget) -> Result<Self> {
        let used = lengths.iter().filter(|&&l| l > 0).count();
        let mut codes: Vec<u32> = budget.with_capacity(used)?;
        let mut entry_of: Vec<u32> = budget.with_capacity(used)?;
        // libvorbis's _make_words, with 64-bit markers so that a length of 32 is checked too.
        let mut marker = [0u64; 33];
        for (e, &len) in lengths.iter().enumerate() {
            if len == 0 {
                continue;
            }
            let len = usize::from(len);
            let mut entry = marker[len];
            if entry >> len != 0 {
                return Err(malformed("overspecified Huffman tree"));
            }
            codes.push(entry as u32);
            entry_of.push(e as u32);
            for j in (1..=len).rev() {
                if marker[j] & 1 != 0 {
                    marker[j] = if j == 1 {
                        marker[1] + 1
                    } else {
                        marker[j - 1] << 1
                    };
                    break;
                }
                marker[j] += 1;
            }
            for j in len + 1..33 {
                if marker[j] >> 1 != entry {
                    break;
                }
                entry = marker[j];
                marker[j] = marker[j - 1] << 1;
            }
        }
        // A single codeword of length 1 is libvorbis's one allowed incomplete tree.
        let single = used == 1 && marker[2] == 2;
        if !single && (1..33).any(|i| marker[i] & ((1u64 << i) - 1) != 0) {
            return Err(malformed("underspecified Huffman tree"));
        }

        let max_len = lengths.iter().copied().max().unwrap_or(0);
        let table_bits = u32::from(max_len).clamp(1, TABLE_BITS);
        let mut table = budget.vec(1usize << table_bits, 0u32)?;
        if single {
            // Either bit decodes as the one entry, as in libvorbis.
            table.fill(1);
        }
        let long = codes
            .iter()
            .zip(&entry_of)
            .filter(|&(_, &e)| u32::from(lengths[e as usize]) > table_bits)
            .count();
        let mut long_pairs: Vec<(u32, u32)> = budget.with_capacity(long)?;
        if !single {
            for (sym, (&code, &e)) in codes.iter().zip(&entry_of).enumerate() {
                let len = u32::from(lengths[e as usize]);
                let info = ((sym as u32) << 6) | len;
                if len <= table_bits {
                    let rev = code.reverse_bits() >> (32 - len);
                    let mut i = rev as usize;
                    while i < table.len() {
                        table[i] = info;
                        i += 1 << len;
                    }
                } else {
                    long_pairs.push((code << (32 - len), info));
                }
            }
        }
        long_pairs.sort_unstable();
        let mut long_codes = budget.with_capacity(long)?;
        let mut long_info = budget.with_capacity(long)?;
        for (c, i) in long_pairs {
            long_codes.push(c);
            long_info.push(i);
        }
        Ok(Self {
            dims: 1,
            entries: lengths.len() as u32,
            lookup: false,
            table_bits,
            table,
            long_codes,
            long_info,
            entry_of,
            vq: Vec::new(),
        })
    }

    pub(crate) fn dims(&self) -> usize {
        self.dims
    }

    pub(crate) fn entries(&self) -> u32 {
        self.entries
    }

    /// Whether the book has vector values (lookup type 1 or 2).
    pub(crate) fn has_lookup(&self) -> bool {
        self.lookup
    }

    /// The vector values, `dims` per symbol.
    pub(crate) fn vq(&self) -> &[f32] {
        &self.vq
    }

    /// The bytes the book holds on the heap.
    pub(crate) fn heap_bytes(&self) -> usize {
        self.table.capacity() * 4
            + self.long_codes.capacity() * 4
            + self.long_info.capacity() * 4
            + self.entry_of.capacity() * 4
            + self.vq.capacity() * 4
    }

    /// The next symbol (index among the used entries), or `None` at the end of the packet or
    /// for a book with no used entry (which reads nothing).
    #[inline(always)]
    pub(crate) fn decode_symbol(&self, r: &mut BitReader<'_>) -> Option<u32> {
        let slot = r.peek(self.table_bits) as usize & (self.table.len() - 1);
        let info = self.table[slot];
        if info != 0 {
            return r.consume(info & 63).then_some(info >> 6);
        }
        self.decode_long(r)
    }

    #[inline(never)]
    fn decode_long(&self, r: &mut BitReader<'_>) -> Option<u32> {
        let v = r.peek(32).reverse_bits();
        let i = self
            .long_codes
            .partition_point(|&c| c <= v)
            .checked_sub(1)?;
        let info = self.long_info[i];
        let len = info & 63;
        if (u64::from(v ^ self.long_codes[i]) >> (32 - len)) != 0 {
            return None;
        }
        r.consume(len).then_some(info >> 6)
    }

    /// The next entry number (scalar context).
    #[inline]
    pub(crate) fn decode_entry(&self, r: &mut BitReader<'_>) -> Option<u32> {
        self.decode_symbol(r).map(|s| self.entry_of[s as usize])
    }

    /// The next vector (VQ context); the book must have a lookup.
    #[inline]
    pub(crate) fn decode_vector(&self, r: &mut BitReader<'_>) -> Option<&[f32]> {
        let s = self.decode_symbol(r)? as usize;
        self.vq.get(s * self.dims..(s + 1) * self.dims)
    }
}

#[allow(clippy::too_many_arguments)]
fn unquantize(
    entry_of: &[u32],
    dims: usize,
    kind: u32,
    minimum: f32,
    delta: f32,
    sequence: bool,
    multiplicands: &[u32],
    budget: &mut Budget,
) -> Result<Vec<f32>> {
    let mut vq = budget.with_capacity(entry_of.len().saturating_mul(dims))?;
    let values = multiplicands.len() as u64;
    for &e in entry_of {
        let mut last = 0.0f32;
        let mut divisor = 1u64;
        for i in 0..dims {
            let at = if kind == 1 {
                (u64::from(e) / divisor % values) as usize
            } else {
                e as usize * dims + i
            };
            let v = multiplicands[at] as f32 * delta + minimum + last;
            if sequence {
                last = v;
            }
            vq.push(v);
            divisor = divisor.saturating_mul(values);
        }
    }
    Ok(vq)
}

/// Vorbis's packed float: a 21-bit mantissa, a sign and a 10-bit exponent biased by 788,
/// unpacked exactly (an exponent too large for `f32` gives infinity, too small gives 0).
pub(crate) fn float32_unpack(x: u32) -> f32 {
    let mantissa = f64::from(x & 0x1F_FFFF);
    let exponent = ((x & 0x7FE0_0000) >> 21) as i32;
    let v = libm::ldexp(mantissa, exponent - 788);
    (if x & 0x8000_0000 != 0 { -v } else { v }) as f32
}

/// The greatest `r` with `r^dims <= entries`.
pub(crate) fn lookup1_values(entries: u32, dims: u16) -> u32 {
    let fits = |r: u32| {
        u64::from(r)
            .checked_pow(u32::from(dims))
            .is_some_and(|p| p <= u64::from(entries))
    };
    let (mut lo, mut hi) = (0u32, entries);
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        if fits(mid) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

#[cfg(test)]
#[path = "codebook_tests.rs"]
mod tests;
