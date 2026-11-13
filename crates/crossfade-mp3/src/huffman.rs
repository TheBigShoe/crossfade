//! Huffman decoding of the big-values and count1 regions (ISO/IEC 11172-3 2.4.2.7, Annex B
//! Table B.7). The code trees are minimp3's compact form (see `tables`): a lookup of a few bits
//! gives either a leaf (the pair and its code length) or a jump into a sub-table.

use crate::bits::Bits;
use crate::sideinfo::Granule;
use crate::tables::{
    HUFFMAN_LINBITS, HUFFMAN_TAB32, HUFFMAN_TAB33, HUFFMAN_TABINDEX, HUFFMAN_TABS,
};

/// Bits a first lookup takes: codes up to this long decode in one step.
const FAST_BITS: u32 = 10;

/// Per code tree, every `FAST_BITS`-bit prefix's code when it is that short: `0x8000 | length
/// << 8 | values` (the pair as two nibbles), or 0 for a longer code, which walks the tree.
pub(crate) struct Fast {
    tables: Vec<[u16; 1 << FAST_BITS]>,
    /// Huffman table number to its entry in `tables`.
    index: [usize; 32],
}

impl Fast {
    pub(crate) fn new() -> Self {
        let mut tables = Vec::new();
        let mut index = [0; 32];
        let mut built: Vec<i16> = Vec::new();
        for (table, &start) in HUFFMAN_TABINDEX.iter().enumerate() {
            if let Some(i) = built.iter().position(|&s| s == start) {
                index[table] = i;
                continue;
            }
            let tree = &HUFFMAN_TABS[start as usize..];
            let mut fast = [0u16; 1 << FAST_BITS];
            for (prefix, entry) in fast.iter_mut().enumerate() {
                let data = ((prefix as u64) << (64 - FAST_BITS)).to_be_bytes();
                let mut b = Bits::new(&data);
                let leaf = walk(&mut b, tree);
                let length = b.pos() as u32;
                if length <= FAST_BITS {
                    *entry = 0x8000 | (length << 8) as u16 | (leaf & 0xFF) as u16;
                }
            }
            index[table] = tables.len();
            built.push(start);
            tables.push(fast);
        }
        Self { tables, index }
    }
}

/// One code from the tree, consumed: its pair as two nibbles.
#[inline(always)]
fn walk(b: &mut Bits<'_>, tree: &[i16]) -> i32 {
    let mut w = 5;
    let mut leaf = i32::from(tree.get(b.peek(w) as usize).copied().unwrap_or(0));
    while leaf < 0 {
        b.skip(w);
        w = (leaf & 7) as u32;
        let at = (b.peek(w) as i32).wrapping_sub(leaf >> 3) as usize;
        leaf = i32::from(tree.get(at).copied().unwrap_or(0));
    }
    b.skip((leaf >> 8) as u32);
    leaf & 0xFF
}

/// Decodes the granule's spectral values as signed `|v|^(4/3)` into `out` (not yet scaled by
/// the scalefactors), reading up to bit `end` of `b`. Returns how many lines from the start may
/// be non-zero; the rest are zeroed.
pub(crate) fn decode(
    b: &mut Bits<'_>,
    g: &Granule,
    end: usize,
    pow43: &[f32],
    fast: &Fast,
    out: &mut [f32; 576],
) -> usize {
    // A copy whose address never escapes, so its position and cache stay in registers.
    let mut r = b.clone();
    let n = decode_with(&mut r, g, end, pow43, fast, out);
    *b = r;
    n
}

#[inline(always)]
fn decode_with(
    b: &mut Bits<'_>,
    g: &Granule,
    end: usize,
    pow43: &[f32],
    fast: &Fast,
    out: &mut [f32; 576],
) -> usize {
    let big = (usize::from(g.big_values) * 2).min(576);
    let r1 = usize::from(g.region1).min(big);
    let r2 = usize::from(g.region2).clamp(r1, big);
    let mut line = 0;
    for (region_end, &table) in [r1, r2, big].iter().zip(&g.table_select) {
        let pairs = &mut out[line..*region_end];
        pairs_of(b, usize::from(table), pow43, fast, pairs);
        line = *region_end;
    }
    let n = count1(b, g.count1_table, end, &mut out[big..]) + big;
    out[n..].fill(0.0);
    n
}

/// Big values with one table, two lines at a time.
#[inline(always)]
fn pairs_of(b: &mut Bits<'_>, table: usize, pow43: &[f32], fast: &Fast, out: &mut [f32]) {
    let linbits = u32::from(HUFFMAN_LINBITS[table]);
    let tree = &HUFFMAN_TABS[HUFFMAN_TABINDEX[table] as usize..];
    let fast = &fast.tables[fast.index[table]];
    if linbits == 0 {
        pairs::<false>(b, tree, fast, 0, pow43, out);
    } else {
        pairs::<true>(b, tree, fast, linbits, pow43, out);
    }
}

#[inline(always)]
fn pairs<const LINBITS: bool>(
    b: &mut Bits<'_>,
    tree: &[i16],
    fast: &[u16; 1 << FAST_BITS],
    linbits: u32,
    pow43: &[f32],
    out: &mut [f32],
) {
    for pair in out.as_chunks_mut::<2>().0 {
        b.refill();
        let entry = fast[b.peek(FAST_BITS) as usize];
        let mut leaf = if entry != 0 {
            b.skip(u32::from(entry >> 8 & 0x1F));
            i32::from(entry & 0xFF)
        } else {
            walk(b, tree)
        };
        for v in pair {
            let mut mag = (leaf & 15) as u32;
            leaf >>= 4;
            if LINBITS && mag == 15 {
                mag += b.peek(linbits);
                b.skip(linbits);
            }
            // Without branches: whether a value is zero is as good as random, and a sign bit
            // follows only a non-zero one. `pow43[0]` is 0.
            let nonzero = u32::from(mag != 0);
            let sign = u32::from(b.top()) & nonzero;
            b.skip(nonzero);
            let x = pow43.get(mag as usize).copied().unwrap_or(0.0);
            *v = f32::from_bits(x.to_bits() ^ (sign << 31));
        }
    }
}

/// The count1 region: quadruples of -1, 0 or 1 until bit `end`. Returns the lines written.
fn count1(b: &mut Bits<'_>, table_b: bool, end: usize, out: &mut [f32]) -> usize {
    let tab: &[u8] = if table_b {
        &HUFFMAN_TAB33
    } else {
        &HUFFMAN_TAB32
    };
    let mut i = 0;
    while i < out.len() {
        b.refill();
        let mut leaf = tab[b.peek(4) as usize];
        if leaf & 8 == 0 {
            let extra = u32::from(leaf & 3);
            let at = usize::from(leaf >> 3)
                + ((b.cache() << 4).checked_shr(64 - extra).unwrap_or(0)) as usize;
            leaf = tab.get(at).copied().unwrap_or(0);
        }
        b.skip(u32::from(leaf & 7));
        // A quadruple whose code runs past the granule's data is not part of it.
        if b.pos() > end {
            break;
        }
        for s in 0..4 {
            let v = if leaf & (128 >> s) != 0 {
                let neg = b.top();
                b.skip(1);
                if neg { -1.0 } else { 1.0 }
            } else {
                0.0
            };
            if let Some(o) = out.get_mut(i + s) {
                *o = v;
            }
        }
        i += 4;
    }
    i.min(out.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_lookup_agrees_with_the_tree_for_every_table() {
        let fast = Fast::new();
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        for table in 0..32 {
            let tree = &HUFFMAN_TABS[HUFFMAN_TABINDEX[table] as usize..];
            let lookup = &fast.tables[fast.index[table]];
            let mut short = 0;
            for _ in 0..20_000 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let data = seed.to_be_bytes();
                let mut b = Bits::new(&data);
                let leaf = walk(&mut b, tree);
                let entry = lookup[(seed >> (64 - FAST_BITS)) as usize];
                if b.pos() <= FAST_BITS as usize {
                    short += 1;
                    let want = 0x8000 | (b.pos() as u16) << 8 | leaf as u16;
                    assert_eq!(entry, want, "table {table}, bits {seed:016x}");
                } else {
                    assert_eq!(entry, 0, "table {table}, bits {seed:016x}");
                }
            }
            assert!(short > 0, "table {table}");
        }
    }
}
