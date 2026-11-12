//! Scalefactors (ISO/IEC 11172-3 2.4.2.7 and 2.4.3.4.5, 13818-3 2.4.3.2).

use crate::bits::Bits;
use crate::sideinfo::{Granule, SHORT_BLOCK};
use crate::tables::{LSF_PARTITIONS, SLEN};

/// One granule-channel's scalefactors, one per entry of its `Bands`, with the bit width each
/// was coded in (intensity stereo needs it: MPEG-2's illegal position is the largest value).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Scalefactors {
    pub(crate) value: [u8; 40],
    pub(crate) slen: [u8; 40],
}

impl Scalefactors {
    pub(crate) const ZERO: Self = Self {
        value: [0; 40],
        slen: [0; 40],
    };
}

/// MPEG-1: reads the scalefactors of granule `gr`; `previous` is granule 0's of the same
/// channel, whose band groups `scfsi` says to reuse in granule 1.
pub(crate) fn read_mpeg1(
    b: &mut Bits<'_>,
    g: &Granule,
    gr: usize,
    previous: &Scalefactors,
) -> Scalefactors {
    let (slen1, slen2) = SLEN[usize::from(g.scalefac_compress & 15)];
    let mut sf = Scalefactors::ZERO;
    if g.block_type == SHORT_BLOCK {
        let n_long = usize::from(g.bands.n_long);
        // Long bands of a mixed block and short bands 0-5 use slen1, bands 6-11 slen2.
        let first = n_long + if g.mixed { 9 } else { 18 };
        for k in 0..first + 18 {
            let bits = if k < first { slen1 } else { slen2 };
            sf.value[k] = b.read(u32::from(bits)) as u8;
            sf.slen[k] = bits;
        }
        return sf;
    }
    const GROUPS: [(usize, usize); 4] = [(0, 6), (6, 11), (11, 16), (16, 21)];
    for (i, &(from, to)) in GROUPS.iter().enumerate() {
        let bits = if i < 2 { slen1 } else { slen2 };
        if gr == 1 && g.scfsi & (8 >> i) != 0 {
            sf.value[from..to].copy_from_slice(&previous.value[from..to]);
            sf.slen[from..to].copy_from_slice(&previous.slen[from..to]);
        } else {
            for k in from..to {
                sf.value[k] = b.read(u32::from(bits)) as u8;
                sf.slen[k] = bits;
            }
        }
    }
    sf
}

/// MPEG-2 and 2.5: reads the scalefactors; `is_right` is the right channel of an intensity
/// stereo frame, whose `scalefac_compress` is coded differently.
pub(crate) fn read_lsf(b: &mut Bits<'_>, g: &Granule, is_right: bool) -> Scalefactors {
    let sfc = usize::from(g.scalefac_compress);
    let (slen, row) = if !is_right {
        if sfc < 400 {
            (
                [(sfc >> 4) / 5, (sfc >> 4) % 5, (sfc & 15) >> 2, sfc & 3],
                0,
            )
        } else if sfc < 500 {
            let s = sfc - 400;
            ([(s >> 2) / 5, (s >> 2) % 5, s & 3, 0], 1)
        } else {
            let s = sfc - 500;
            ([s / 3, s % 3, 0, 0], 2)
        }
    } else {
        let isc = sfc >> 1;
        if isc < 180 {
            ([isc / 36, (isc % 36) / 6, isc % 6, 0], 3)
        } else if isc < 244 {
            let s = isc - 180;
            ([(s & 63) >> 4, (s & 15) >> 2, s & 3, 0], 4)
        } else {
            let s = isc - 244;
            ([s / 3, s % 3, 0, 0], 5)
        }
    };
    let kind = match (g.block_type == SHORT_BLOCK, g.mixed) {
        (false, _) => 0,
        (true, false) => 1,
        (true, true) => 2,
    };
    let mut sf = Scalefactors::ZERO;
    let mut k = 0;
    for (part, &count) in LSF_PARTITIONS[row][kind].iter().enumerate() {
        let bits = slen[part] as u8;
        for _ in 0..count {
            sf.value[k] = b.read(u32::from(bits)) as u8;
            sf.slen[k] = bits;
            k += 1;
        }
    }
    sf
}
