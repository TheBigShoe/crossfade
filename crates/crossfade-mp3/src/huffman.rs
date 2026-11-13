//! Huffman decoding of the big-values and count1 regions (ISO/IEC 11172-3 2.4.2.7, Annex B
//! Table B.7). The code trees are minimp3's compact form (see `tables`): a lookup of a few bits
//! gives either a leaf (the pair and its code length) or a jump into a sub-table.

use crate::bits::Bits;
use crate::sideinfo::Granule;
use crate::tables::{
    HUFFMAN_LINBITS, HUFFMAN_TAB32, HUFFMAN_TAB33, HUFFMAN_TABINDEX, HUFFMAN_TABS,
};

/// Decodes the granule's spectral values as signed `|v|^(4/3)` into `out` (not yet scaled by
/// the scalefactors), reading up to bit `end` of `b`. Returns how many lines from the start may
/// be non-zero; the rest are zeroed.
pub(crate) fn decode(
    b: &mut Bits<'_>,
    g: &Granule,
    end: usize,
    pow43: &[f32],
    out: &mut [f32; 576],
) -> usize {
    let big = (usize::from(g.big_values) * 2).min(576);
    let r1 = usize::from(g.region1).min(big);
    let r2 = usize::from(g.region2).clamp(r1, big);
    let mut line = 0;
    for (region_end, &table) in [r1, r2, big].iter().zip(&g.table_select) {
        let pairs = &mut out[line..*region_end];
        pairs_of(b, usize::from(table), pow43, pairs);
        line = *region_end;
    }
    let n = count1(b, g.count1_table, end, &mut out[big..]) + big;
    out[n..].fill(0.0);
    n
}

/// Big values with one table, two lines at a time.
#[inline(always)]
fn pairs_of(b: &mut Bits<'_>, table: usize, pow43: &[f32], out: &mut [f32]) {
    let linbits = u32::from(HUFFMAN_LINBITS[table]);
    let tree = &HUFFMAN_TABS[HUFFMAN_TABINDEX[table] as usize..];
    if linbits == 0 {
        pairs::<false>(b, tree, 0, pow43, out);
    } else {
        pairs::<true>(b, tree, linbits, pow43, out);
    }
}

#[inline(always)]
fn pairs<const LINBITS: bool>(
    b: &mut Bits<'_>,
    tree: &[i16],
    linbits: u32,
    pow43: &[f32],
    out: &mut [f32],
) {
    for pair in out.as_chunks_mut::<2>().0 {
        b.refill();
        let mut w = 5;
        let mut leaf = i32::from(tree.get(b.peek(w) as usize).copied().unwrap_or(0));
        while leaf < 0 {
            b.skip(w);
            w = (leaf & 7) as u32;
            let at = (b.peek(w) as i32).wrapping_sub(leaf >> 3) as usize;
            leaf = i32::from(tree.get(at).copied().unwrap_or(0));
        }
        b.skip((leaf >> 8) as u32);
        for v in pair {
            let mut mag = (leaf & 15) as u32;
            leaf >>= 4;
            if LINBITS && mag == 15 {
                mag += b.peek(linbits);
                b.skip(linbits);
            }
            if mag == 0 {
                *v = 0.0;
            } else {
                let neg = b.top();
                b.skip(1);
                let x = pow43[mag as usize];
                *v = if neg { -x } else { x };
            }
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
