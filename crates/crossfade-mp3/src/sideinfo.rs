//! Layer III side information (ISO/IEC 11172-3 2.4.1.7, 13818-3 2.4.1.7).

use crate::bits::Bits;
use crate::tables::{LONG_WIDTHS, SHORT_WIDTHS, rate_index};
use crate::{Error, FrameHeader};

pub(crate) const SHORT_BLOCK: u8 = 2;
pub(crate) const STOP_BLOCK: u8 = 3;

/// The scalefactor bands of one granule in bitstream order: long bands, then (short or mixed
/// blocks) each short band once per window. `width[k]` lines each; a zero ends the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Bands {
    pub(crate) width: [u8; 40],
    /// Long bands at the start (22 for long blocks, 8 or 6 for mixed, 0 for short).
    pub(crate) n_long: u8,
    /// (band, window) entries after them: 39 short, 30 mixed, 0 long.
    pub(crate) n_short: u8,
    /// Subbands of 18 lines the long bands of a mixed block cover (2, or 4 at 8 kHz).
    pub(crate) long_subbands: u8,
}

impl Bands {
    fn new(rate: usize, block_type: u8, mixed: bool, lsf: bool) -> Self {
        let mut width = [0u8; 40];
        // A mixed block's long part is the long bands up to line 36 (MPEG-2.5 at 8 kHz:
        // line 72, where its doubled bands put the same boundary); its short part starts at
        // the short band that begins there, band 3. Whatever the block type, the subbands of
        // the long part get the normal window.
        let n_long = if !mixed {
            0
        } else if lsf {
            6
        } else {
            8
        };
        let long_lines: usize = LONG_WIDTHS[rate][..n_long]
            .iter()
            .map(|&w| usize::from(w))
            .sum();
        if block_type != SHORT_BLOCK {
            width[..22].copy_from_slice(&LONG_WIDTHS[rate]);
            return Self {
                width,
                n_long: 22,
                n_short: 0,
                long_subbands: (long_lines / 18) as u8,
            };
        }
        width[..n_long].copy_from_slice(&LONG_WIDTHS[rate][..n_long]);
        let first_short = if mixed { 3 } else { 0 };
        let mut k = n_long;
        for &w in &SHORT_WIDTHS[rate][first_short..] {
            width[k..k + 3].fill(w);
            k += 3;
        }
        Self {
            width,
            n_long: n_long as u8,
            n_short: (k - n_long) as u8,
            long_subbands: (long_lines / 18) as u8,
        }
    }

    pub(crate) fn count(&self) -> usize {
        usize::from(self.n_long) + usize::from(self.n_short)
    }
}

/// One granule of one channel.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Granule {
    pub(crate) part2_3_length: u16,
    pub(crate) big_values: u16,
    pub(crate) global_gain: u8,
    pub(crate) scalefac_compress: u16,
    pub(crate) block_type: u8,
    pub(crate) mixed: bool,
    pub(crate) table_select: [u8; 3],
    pub(crate) subblock_gain: [u8; 3],
    /// Lines where regions 1 and 2 of the big values start.
    pub(crate) region1: u16,
    pub(crate) region2: u16,
    pub(crate) preflag: bool,
    pub(crate) scalefac_scale: bool,
    pub(crate) count1_table: bool,
    /// MPEG-1: which of the four band groups reuse granule 0's scalefactors (bit 3 is group 0).
    pub(crate) scfsi: u8,
    pub(crate) bands: Bands,
}

/// The side information of one frame.
pub(crate) struct SideInfo {
    pub(crate) main_data_begin: usize,
    /// `[granule][channel]`; MPEG-2 and 2.5 have one granule.
    pub(crate) granules: [[Granule; 2]; 2],
    pub(crate) n_granules: usize,
}

impl SideInfo {
    /// Parses the side information at the start of `data` (after the header and any CRC).
    pub(crate) fn parse(h: &FrameHeader, data: &[u8]) -> Result<Self, Error> {
        let lsf = h.version.is_lsf();
        let nch = usize::from(h.channels());
        let rate = rate_index(h.version, h.sample_rate);
        let mut b = Bits::new(data);
        let main_data_begin;
        let mut scfsi = [0u8; 2];
        if lsf {
            main_data_begin = b.read(8) as usize;
            b.read(if nch == 1 { 1 } else { 2 });
        } else {
            main_data_begin = b.read(9) as usize;
            b.read(if nch == 1 { 5 } else { 3 });
            for s in scfsi.iter_mut().take(nch) {
                *s = b.read(4) as u8;
            }
        }
        let n_granules = if lsf { 1 } else { 2 };
        let blank = Granule {
            part2_3_length: 0,
            big_values: 0,
            global_gain: 0,
            scalefac_compress: 0,
            block_type: 0,
            mixed: false,
            table_select: [0; 3],
            subblock_gain: [0; 3],
            region1: 576,
            region2: 576,
            preflag: false,
            scalefac_scale: false,
            count1_table: false,
            scfsi: 0,
            bands: Bands::new(rate, 0, false, lsf),
        };
        let mut granules = [[blank; 2]; 2];
        for gr in granules.iter_mut().take(n_granules) {
            for (ch, g) in gr.iter_mut().enumerate().take(nch) {
                g.part2_3_length = b.read(12) as u16;
                g.big_values = b.read(9) as u16;
                if g.big_values > 288 {
                    return Err(Error::Corrupt("big_values above 288"));
                }
                g.global_gain = b.read(8) as u8;
                g.scalefac_compress = b.read(if lsf { 9 } else { 4 }) as u16;
                let long = &LONG_WIDTHS[rate];
                let line =
                    |bands: usize| -> u16 { long.iter().take(bands).map(|&w| u16::from(w)).sum() };
                if b.read(1) == 1 {
                    g.block_type = b.read(2) as u8;
                    if g.block_type == 0 {
                        return Err(Error::Corrupt("window switching with block type 0"));
                    }
                    g.mixed = b.read(1) == 1;
                    g.table_select = [b.read(5) as u8, b.read(5) as u8, 0];
                    g.subblock_gain = [b.read(3) as u8, b.read(3) as u8, b.read(3) as u8];
                    g.bands = Bands::new(rate, g.block_type, g.mixed, lsf);
                    g.region1 = if g.block_type == SHORT_BLOCK {
                        if g.mixed {
                            u16::from(g.bands.long_subbands) * 18
                        } else {
                            3 * line_short(rate, 3)
                        }
                    } else {
                        line(8)
                    };
                    g.region2 = 576;
                } else {
                    g.block_type = 0;
                    g.mixed = false;
                    g.table_select = [b.read(5) as u8, b.read(5) as u8, b.read(5) as u8];
                    let r0 = b.read(4) as usize;
                    let r1 = b.read(3) as usize;
                    g.region1 = line(r0 + 1);
                    g.region2 = line(r0 + r1 + 2);
                    g.bands = Bands::new(rate, 0, false, lsf);
                }
                g.preflag = if lsf {
                    // MPEG-2 signals pre-emphasis through scalefac_compress (the right channel
                    // of intensity stereo never has it).
                    g.scalefac_compress >= 500 && !(h.intensity_stereo() && ch == 1)
                } else {
                    b.read(1) == 1
                };
                g.scalefac_scale = b.read(1) == 1;
                g.count1_table = b.read(1) == 1;
                g.scfsi = if g.block_type == SHORT_BLOCK {
                    0
                } else {
                    scfsi[ch]
                };
            }
        }
        if b.pos() > data.len() * 8 {
            return Err(Error::Corrupt("side information cut short"));
        }
        Ok(Self {
            main_data_begin,
            granules,
            n_granules,
        })
    }
}

fn line_short(rate: usize, band: usize) -> u16 {
    SHORT_WIDTHS[rate][..band]
        .iter()
        .map(|&w| u16::from(w))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_layouts_cover_the_granule() {
        for rate in 0..8 {
            for (bt, mixed) in [(0, false), (1, false), (2, false), (2, true), (3, false)] {
                {
                    let lsf = rate < 5;
                    let b = Bands::new(rate, bt, mixed, lsf);
                    let total: usize = b.width.iter().map(|&w| usize::from(w)).sum();
                    assert_eq!(total, 576, "rate {rate} type {bt} mixed {mixed} lsf {lsf}");
                    assert_eq!(b.count(), b.width.iter().take_while(|&&w| w != 0).count());
                    if mixed && bt == 2 {
                        let long: usize = b.width[..usize::from(b.n_long)]
                            .iter()
                            .map(|&w| usize::from(w))
                            .sum();
                        assert_eq!(long, usize::from(b.long_subbands) * 18);
                    }
                }
            }
        }
        // MPEG-2.5 at 8 kHz: the mixed block's long part reaches line 72, four subbands.
        let b = Bands::new(1, 2, true, true);
        assert_eq!((b.n_long, b.n_short, b.long_subbands), (6, 30, 4));
    }
}
