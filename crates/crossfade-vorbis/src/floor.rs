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
