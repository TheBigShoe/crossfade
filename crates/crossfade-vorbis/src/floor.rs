//! Floors (specification sections 6 and 7): their setup.

use crate::bits::BitReader;
use crate::codebook::{Budget, Codebook};
use crate::error::{Result, malformed};

#[derive(Debug, Clone)]
pub(crate) enum Floor {
    Zero(Floor0),
    One(Floor1),
}

#[derive(Debug, Clone)]
pub(crate) struct Floor0 {
    pub(crate) order: usize,
    pub(crate) amplitude_bits: u32,
    pub(crate) amplitude_offset: u32,
    pub(crate) books: Vec<u8>,
    /// cos(omega) of each spectral line, for the short and the long block.
    pub(crate) cos_omega: [Vec<f32>; 2],
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Floor1Class {
    pub(crate) dims: u8,
    pub(crate) sub_bits: u8,
    pub(crate) masterbook: u8,
    /// -1 for none.
    pub(crate) subbooks: [i16; 8],
}

#[derive(Debug, Clone)]
pub(crate) struct Floor1 {
    pub(crate) multiplier: i32,
    /// 256, 128, 86 or 64.
    pub(crate) range: i32,
    pub(crate) partitions: Vec<u8>,
    pub(crate) classes: Vec<Floor1Class>,
    /// Every post's X, the implicit 0 and `1 << range_bits` first.
    pub(crate) xs: Vec<u16>,
    /// Post numbers in order of X.
    pub(crate) sorted: Vec<u8>,
    /// The low and high neighbour of each post from the third on.
    pub(crate) neighbors: Vec<(u8, u8)>,
}

/// Vorbis I allows at most 65 posts (libvorbis's VIF_POSIT + 2).
const MAX_POSTS: usize = 65;

pub(crate) fn parse(
    r: &mut BitReader<'_>,
    books: &[Codebook],
    block_sizes: [u16; 2],
    budget: &mut Budget,
) -> Result<Floor> {
    let eof = || malformed("setup header ends inside a floor");
    let book = |b: u32, what: &str| -> Result<u8> {
        if b as usize >= books.len() {
            return Err(malformed(format!(
                "{what} uses codebook {b}, which does not exist"
            )));
        }
        Ok(b as u8)
    };
    match r.read(16).ok_or_else(eof)? {
        0 => {
            let order = r.read(8).ok_or_else(eof)?;
            let rate = r.read(16).ok_or_else(eof)?;
            let bark_map_size = r.read(16).ok_or_else(eof)?;
            let amplitude_bits = r.read(6).ok_or_else(eof)?;
            let amplitude_offset = r.read(8).ok_or_else(eof)?;
            let count = r.read(4).ok_or_else(eof)? + 1;
            let mut list = budget.with_capacity(count as usize)?;
            for _ in 0..count {
                let b = book(r.read(8).ok_or_else(eof)?, "floor 0")?;
                if !books[usize::from(b)].has_lookup() {
                    return Err(malformed(format!(
                        "floor 0 codebook {b} has no values to read vectors from"
                    )));
                }
                list.push(b);
            }
            // Each would divide by zero or never end in the curve computation.
            if order == 0 {
                return Err(malformed("floor 0 order of 0"));
            }
            if rate == 0 {
                return Err(malformed("floor 0 rate of 0"));
            }
            if bark_map_size == 0 {
                return Err(malformed("floor 0 bark map size of 0"));
            }
            let cos_omega = [
                bark_cos_omega(usize::from(block_sizes[0]) / 2, rate, bark_map_size, budget)?,
                bark_cos_omega(usize::from(block_sizes[1]) / 2, rate, bark_map_size, budget)?,
            ];
            Ok(Floor::Zero(Floor0 {
                order: order as usize,
                amplitude_bits,
                amplitude_offset,
                books: list,
                cos_omega,
            }))
        }
        1 => {
            let count = r.read(5).ok_or_else(eof)? as usize;
            let mut partitions = budget.with_capacity(count)?;
            for _ in 0..count {
                partitions.push(r.read(4).ok_or_else(eof)? as u8);
            }
            let class_count = partitions
                .iter()
                .map(|&c| usize::from(c) + 1)
                .max()
                .unwrap_or(0);
            let mut classes = budget.with_capacity(class_count)?;
            for _ in 0..class_count {
                let dims = r.read(3).ok_or_else(eof)? as u8 + 1;
                let sub_bits = r.read(2).ok_or_else(eof)? as u8;
                let masterbook = if sub_bits > 0 {
                    book(r.read(8).ok_or_else(eof)?, "floor 1")?
                } else {
                    0
                };
                let mut subbooks = [-1i16; 8];
                for s in subbooks.iter_mut().take(1 << sub_bits) {
                    let b = r.read(8).ok_or_else(eof)?;
                    if b > 0 {
                        *s = i16::from(book(b - 1, "floor 1")?);
                    }
                }
                classes.push(Floor1Class {
                    dims,
                    sub_bits,
                    masterbook,
                    subbooks,
                });
            }
            let multiplier = r.read(2).ok_or_else(eof)? as i32 + 1;
            let range_bits = r.read(4).ok_or_else(eof)?;
            let mut xs = budget.with_capacity(MAX_POSTS)?;
            xs.extend([0u16, 1 << range_bits]);
            for &p in &partitions {
                for _ in 0..classes[usize::from(p)].dims {
                    if xs.len() == MAX_POSTS {
                        return Err(malformed("floor 1 has more than 65 posts"));
                    }
                    xs.push(r.read(range_bits).ok_or_else(eof)? as u16);
                }
            }
            let mut sorted: Vec<u8> = budget.with_capacity(xs.len())?;
            sorted.extend(0..xs.len() as u8);
            sorted.sort_by_key(|&i| xs[usize::from(i)]);
            if let Some(w) = sorted
                .windows(2)
                .find(|w| xs[usize::from(w[0])] == xs[usize::from(w[1])])
            {
                return Err(malformed(format!(
                    "floor 1 has the X value {} twice",
                    xs[usize::from(w[0])]
                )));
            }
            let mut neighbors = budget.with_capacity(xs.len())?;
            for i in 2..xs.len() {
                let x = xs[i];
                let (mut lo, mut hi) = (0usize, 1usize);
                for (j, &xj) in xs[..i].iter().enumerate() {
                    if xj < x && xj > xs[lo] {
                        lo = j;
                    }
                    if xj > x && xj < xs[hi] {
                        hi = j;
                    }
                }
                neighbors.push((lo as u8, hi as u8));
            }
            Ok(Floor::One(Floor1 {
                multiplier,
                range: [256, 128, 86, 64][(multiplier - 1) as usize],
                partitions,
                classes,
                xs,
                sorted,
                neighbors,
            }))
        }
        t => Err(malformed(format!("floor type {t}"))),
    }
}

impl Floor {
    #[cfg(test)]
    pub(crate) fn heap_bytes(&self) -> usize {
        match self {
            Floor::Zero(f) => {
                f.books.capacity() + 4 * (f.cos_omega[0].capacity() + f.cos_omega[1].capacity())
            }
            Floor::One(f) => {
                f.partitions.capacity()
                    + f.classes.capacity() * size_of::<Floor1Class>()
                    + f.xs.capacity() * 2
                    + f.sorted.capacity()
                    + f.neighbors.capacity() * 2
            }
        }
    }
}

fn bark(x: f32) -> f32 {
    13.1 * libm::atanf(0.000_74 * x) + 2.24 * libm::atanf(0.000_000_018_5 * x * x) + 0.0001 * x
}

/// cos(omega) of each of `n` lines (section 6.2.3's bark map), computed in `f32` as libvorbis
/// and lewton do.
fn bark_cos_omega(
    n: usize,
    rate: u32,
    bark_map_size: u32,
    budget: &mut Budget,
) -> Result<Vec<f32>> {
    let mut out = budget.with_capacity(n)?;
    let half_rate = rate as f32 / 2.0;
    let scale = bark_map_size as f32 / bark(half_rate);
    let last = bark_map_size as f32 - 1.0;
    let step = std::f32::consts::PI / bark_map_size as f32;
    let hz_per_line = half_rate / n as f32;
    for i in 0..n {
        let map = libm::floorf(bark(i as f32 * hz_per_line) * scale).min(last);
        out.push(libm::cosf(map * step));
    }
    Ok(out)
}

impl Floor0 {
    /// Reads one channel's floor: the amplitude and `order` LSP coefficients, into `coeffs`.
    /// `None` when the floor is unused: an amplitude of zero, a book number past the list, or
    /// the end of the packet (libvorbis treats all three alike).
    pub(crate) fn decode(
        &self,
        books: &[Codebook],
        r: &mut BitReader<'_>,
        coeffs: &mut [f32],
    ) -> Option<u64> {
        let amp = r.read_wide(self.amplitude_bits)?;
        if amp == 0 {
            return None;
        }
        let count = self.books.len() as u32;
        let number = r.read(crate::bits::ilog(count))?;
        let book = &books[usize::from(*self.books.get(number as usize)?)];
        let coeffs = &mut coeffs[..self.order];
        if !book.has_used_entries() {
            // libvorbis reads nothing from such a book and takes zeros.
            coeffs.fill(0.0);
            return Some(amp);
        }
        let mut last = 0.0f32;
        let mut i = 0;
        while i < self.order {
            let v = book.decode_vector(r)?;
            let take = v.len().min(self.order - i);
            for (c, &x) in coeffs[i..i + take].iter_mut().zip(v) {
                *c = x + last;
            }
            last = coeffs[i + take - 1];
            i += take;
        }
        Some(amp)
    }

    /// Multiplies `spectrum` (the block's half) by the LSP curve (section 6.2.3), computed in
    /// `f32` as the specification writes it.
    pub(crate) fn apply(&self, long: bool, amp: u64, coeffs: &[f32], spectrum: &mut [f32]) {
        let cos_omega = &self.cos_omega[usize::from(long)];
        let order = self.order;
        let mut cc = [0.0f32; 256];
        for (c, &x) in cc.iter_mut().zip(&coeffs[..order]) {
            *c = libm::cosf(x);
        }
        let offset = self.amplitude_offset as f32;
        let common = amp as f32 * offset / ((1u64 << self.amplitude_bits) - 1) as f32;
        let n = spectrum.len().min(cos_omega.len());
        let mut i = 0;
        while i < n {
            let w = cos_omega[i];
            let (mut p, mut q, pn, qn) = if order % 2 == 1 {
                (1.0 - w * w, 0.25, (order - 1) / 2, order.div_ceil(2))
            } else {
                ((1.0 - w) / 2.0, (1.0 + w) / 2.0, order / 2, order / 2)
            };
            for j in 0..pn {
                let pm = cc[2 * j + 1] - w;
                p *= 4.0 * pm * pm;
            }
            for j in 0..qn {
                let qm = cc[2 * j] - w;
                q *= 4.0 * qm * qm;
            }
            let value = libm::expf(0.115_129_25 * (common / libm::sqrtf(p + q) - offset));
            // Lines that share a bark map value share the value; the first always matches.
            while i < n && cos_omega[i] == w {
                spectrum[i] *= value;
                i += 1;
            }
        }
    }
}

impl Floor1 {
    /// Reads one channel's floor into `ys` (the raw value of every post). False when the floor
    /// is unused: its flag is clear, or the packet ends (or a book has no used entry).
    pub(crate) fn decode(&self, books: &[Codebook], r: &mut BitReader<'_>, ys: &mut [i32]) -> bool {
        self.read(books, r, ys).is_some()
    }

    fn read(&self, books: &[Codebook], r: &mut BitReader<'_>, ys: &mut [i32]) -> Option<()> {
        if !r.read_bit()? {
            return None;
        }
        let bits = crate::bits::ilog(self.range as u32 - 1);
        ys[0] = r.read(bits)? as i32;
        ys[1] = r.read(bits)? as i32;
        let mut at = 2;
        for &class in &self.partitions {
            let c = &self.classes[usize::from(class)];
            let mask = (1u32 << c.sub_bits) - 1;
            let mut cval = if c.sub_bits > 0 {
                books[usize::from(c.masterbook)].decode_entry(r)?
            } else {
                0
            };
            for _ in 0..c.dims {
                let book = c.subbooks[(cval & mask) as usize];
                cval >>= c.sub_bits;
                ys[at] = if book >= 0 {
                    books[book as usize].decode_entry(r)? as i32
                } else {
                    0
                };
                at += 1;
            }
        }
        Some(())
    }

    /// Multiplies `spectrum` by the curve through the posts (section 7.2.4), as libvorbis
    /// computes it: final values masked to 15 bits with the flag in bit 15, and each line's
    /// ends clamped to the table.
    pub(crate) fn apply(&self, ys: &[i32], spectrum: &mut [f32]) {
        const UNUSED: i32 = 0x8000;
        let posts = self.xs.len();
        let mut fit = [0i32; MAX_POSTS];
        fit[0] = ys[0];
        fit[1] = ys[1];
        for i in 2..posts {
            let (lo, hi) = self.neighbors[i - 2];
            let (lo, hi) = (usize::from(lo), usize::from(hi));
            let predicted = render_point(
                i32::from(self.xs[lo]),
                fit[lo],
                i32::from(self.xs[hi]),
                fit[hi],
                i32::from(self.xs[i]),
            );
            let val = ys[i];
            if val != 0 {
                let high = self.range - predicted;
                let low = predicted;
                let room = high.min(low) * 2;
                let val = if val >= room {
                    if high > low {
                        val - low
                    } else {
                        -1 - (val - high)
                    }
                } else if val & 1 == 1 {
                    -((val + 1) >> 1)
                } else {
                    val >> 1
                };
                fit[i] = (val + predicted) & 0x7FFF;
                fit[lo] &= 0x7FFF;
                fit[hi] &= 0x7FFF;
            } else {
                fit[i] = predicted | UNUSED;
            }
        }
        let n = spectrum.len();
        let mut lx = 0usize;
        let mut ly = (fit[0] * self.multiplier).clamp(0, 255);
        for &cur in &self.sorted[1..] {
            let cur = usize::from(cur);
            if fit[cur] & UNUSED != 0 {
                continue;
            }
            let hx = usize::from(self.xs[cur]);
            let hy = (fit[cur] * self.multiplier).clamp(0, 255);
            render_line(lx, hx, ly, hy, spectrum);
            lx = hx;
            ly = hy;
        }
        if lx < n {
            let v = INVERSE_DB[ly as usize & 255];
            for s in &mut spectrum[lx..] {
                *s *= v;
            }
        }
    }
}

fn render_point(x0: i32, y0: i32, x1: i32, y1: i32, x: i32) -> i32 {
    let (y0, y1) = (y0 & 0x7FFF, y1 & 0x7FFF);
    let dy = y1 - y0;
    let off = dy.abs() * (x - x0) / (x1 - x0);
    if dy < 0 { y0 - off } else { y0 + off }
}

/// Multiplies `d[x0..min(x1, len)]` by the table along the line from (x0, y0) to (x1, y1).
fn render_line(x0: usize, x1: usize, y0: i32, y1: i32, d: &mut [f32]) {
    let end = x1.min(d.len());
    if x0 >= end {
        return;
    }
    let dy = y1 - y0;
    let adx = (x1 - x0) as i32;
    let d = &mut d[x0..end];
    if dy == 0 {
        let v = INVERSE_DB[y0 as usize & 255];
        for s in d {
            *s *= v;
        }
        return;
    }
    let base = dy / adx;
    let sy = if dy < 0 { base - 1 } else { base + 1 };
    let ady = dy.abs() - (base * adx).abs();
    let mut y = y0;
    let mut err = 0;
    d[0] *= INVERSE_DB[y as usize & 255];
    for s in &mut d[1..] {
        err += ady;
        if err >= adx {
            err -= adx;
            y += sy;
        } else {
            y += base;
        }
        *s *= INVERSE_DB[y as usize & 255];
    }
}

/// floor1_inverse_dB_table, from the specification (section 10.1).
#[rustfmt::skip]
#[allow(clippy::excessive_precision)]
pub(crate) const INVERSE_DB: [f32; 256] = [
    1.0649863e-07, 1.1341951e-07, 1.2079015e-07, 1.2863978e-07,
    1.3699951e-07, 1.4590251e-07, 1.5538408e-07, 1.6548181e-07,
    1.7623575e-07, 1.8768855e-07, 1.9988561e-07, 2.1287530e-07,
    2.2670913e-07, 2.4144197e-07, 2.5713223e-07, 2.7384213e-07,
    2.9163793e-07, 3.1059021e-07, 3.3077411e-07, 3.5226968e-07,
    3.7516214e-07, 3.9954229e-07, 4.2550680e-07, 4.5315863e-07,
    4.8260743e-07, 5.1396998e-07, 5.4737065e-07, 5.8294187e-07,
    6.2082472e-07, 6.6116941e-07, 7.0413592e-07, 7.4989464e-07,
    7.9862701e-07, 8.5052630e-07, 9.0579828e-07, 9.6466216e-07,
    1.0273513e-06, 1.0941144e-06, 1.1652161e-06, 1.2409384e-06,
    1.3215816e-06, 1.4074654e-06, 1.4989305e-06, 1.5963394e-06,
    1.7000785e-06, 1.8105592e-06, 1.9282195e-06, 2.0535261e-06,
    2.1869758e-06, 2.3290978e-06, 2.4804557e-06, 2.6416497e-06,
    2.8133190e-06, 2.9961443e-06, 3.1908506e-06, 3.3982101e-06,
    3.6190449e-06, 3.8542308e-06, 4.1047004e-06, 4.3714470e-06,
    4.6555282e-06, 4.9580707e-06, 5.2802740e-06, 5.6234160e-06,
    5.9888572e-06, 6.3780469e-06, 6.7925283e-06, 7.2339451e-06,
    7.7040476e-06, 8.2047000e-06, 8.7378876e-06, 9.3057248e-06,
    9.9104632e-06, 1.0554501e-05, 1.1240392e-05, 1.1970856e-05,
    1.2748789e-05, 1.3577278e-05, 1.4459606e-05, 1.5399272e-05,
    1.6400004e-05, 1.7465768e-05, 1.8600792e-05, 1.9809576e-05,
    2.1096914e-05, 2.2467911e-05, 2.3928002e-05, 2.5482978e-05,
    2.7139006e-05, 2.8902651e-05, 3.0780908e-05, 3.2781225e-05,
    3.4911534e-05, 3.7180282e-05, 3.9596466e-05, 4.2169667e-05,
    4.4910090e-05, 4.7828601e-05, 5.0936773e-05, 5.4246931e-05,
    5.7772202e-05, 6.1526565e-05, 6.5524908e-05, 6.9783085e-05,
    7.4317983e-05, 7.9147585e-05, 8.4291040e-05, 8.9768747e-05,
    9.5602426e-05, 0.00010181521, 0.00010843174, 0.00011547824,
    0.00012298267, 0.00013097477, 0.00013948625, 0.00014855085,
    0.00015820453, 0.00016848555, 0.00017943469, 0.00019109536,
    0.00020351382, 0.00021673929, 0.00023082423, 0.00024582449,
    0.00026179955, 0.00027881276, 0.00029693158, 0.00031622787,
    0.00033677814, 0.00035866388, 0.00038197188, 0.00040679456,
    0.00043323036, 0.00046138411, 0.00049136745, 0.00052329927,
    0.00055730621, 0.00059352311, 0.00063209358, 0.00067317058,
    0.00071691700, 0.00076350630, 0.00081312324, 0.00086596457,
    0.00092223983, 0.00098217216, 0.0010459992, 0.0011139742,
    0.0011863665, 0.0012634633, 0.0013455702, 0.0014330129,
    0.0015261382, 0.0016253153, 0.0017309374, 0.0018434235,
    0.0019632195, 0.0020908006, 0.0022266726, 0.0023713743,
    0.0025254795, 0.0026895994, 0.0028643847, 0.0030505286,
    0.0032487691, 0.0034598925, 0.0036847358, 0.0039241906,
    0.0041792066, 0.0044507950, 0.0047400328, 0.0050480668,
    0.0053761186, 0.0057254891, 0.0060975636, 0.0064938176,
    0.0069158225, 0.0073652516, 0.0078438871, 0.0083536271,
    0.0088964928, 0.009474637, 0.010090352, 0.010746080,
    0.011444421, 0.012188144, 0.012980198, 0.013823725,
    0.014722068, 0.015678791, 0.016697687, 0.017782797,
    0.018938423, 0.020169149, 0.021479854, 0.022875735,
    0.024362330, 0.025945531, 0.027631618, 0.029427276,
    0.031339626, 0.033376252, 0.035545228, 0.037855157,
    0.040315199, 0.042935108, 0.045725273, 0.048696758,
    0.051861348, 0.055231591, 0.058820850, 0.062643361,
    0.066714279, 0.071049749, 0.075666962, 0.080584227,
    0.085821044, 0.091398179, 0.097337747, 0.10366330,
    0.11039993, 0.11757434, 0.12521498, 0.13335215,
    0.14201813, 0.15124727, 0.16107617, 0.17154380,
    0.18269168, 0.19456402, 0.20720788, 0.22067342,
    0.23501402, 0.25028656, 0.26655159, 0.28387361,
    0.30232132, 0.32196786, 0.34289114, 0.36517414,
    0.38890521, 0.41417847, 0.44109412, 0.46975890,
    0.50028648, 0.53279791, 0.56742212, 0.60429640,
    0.64356699, 0.68538959, 0.72993007, 0.77736504,
    0.82788260, 0.88168307, 0.9389798, 1.0,
];
