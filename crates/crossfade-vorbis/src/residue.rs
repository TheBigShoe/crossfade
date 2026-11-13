//! Residues (specification section 8): their setup.

use crate::bits::BitReader;
use crate::codebook::{Budget, Codebook};
use crate::error::{Result, malformed};

#[derive(Debug, Clone)]
pub(crate) struct Residue {
    pub(crate) kind: u8,
    pub(crate) begin: usize,
    pub(crate) end: usize,
    pub(crate) partition_size: usize,
    pub(crate) classifications: u32,
    pub(crate) classbook: u8,
    /// The book of each pass, per classification.
    pub(crate) books: Vec<[Option<u8>; 8]>,
    /// Class book entries at or past this many hold no valid classification (libvorbis stops
    /// the residue there).
    pub(crate) partvals: u32,
}

pub(crate) fn parse(
    r: &mut BitReader<'_>,
    codebooks: &[Codebook],
    budget: &mut Budget,
) -> Result<Residue> {
    let eof = || malformed("setup header ends inside a residue");
    let kind = r.read(16).ok_or_else(eof)?;
    if kind > 2 {
        return Err(malformed(format!("residue type {kind}")));
    }
    let begin = r.read(24).ok_or_else(eof)?;
    let end = r.read(24).ok_or_else(eof)?;
    let partition_size = r.read(24).ok_or_else(eof)? + 1;
    let classifications = r.read(6).ok_or_else(eof)? + 1;
    let classbook = r.read(8).ok_or_else(eof)?;
    let mut cascade = [0u32; 64];
    for c in cascade.iter_mut().take(classifications as usize) {
        *c = r.read(3).ok_or_else(eof)?;
        if r.read_bit().ok_or_else(eof)? {
            *c |= r.read(5).ok_or_else(eof)? << 3;
        }
    }
    let exists = |b: u32| -> Result<u8> {
        if b as usize >= codebooks.len() {
            return Err(malformed(format!(
                "residue uses codebook {b}, which does not exist"
            )));
        }
        Ok(b as u8)
    };
    let mut books = budget.with_capacity(classifications as usize)?;
    for &c in cascade.iter().take(classifications as usize) {
        let mut row = [None; 8];
        for (pass, slot) in row.iter_mut().enumerate() {
            if c & (1 << pass) != 0 {
                let b = exists(r.read(8).ok_or_else(eof)?)?;
                if !codebooks[usize::from(b)].has_lookup() {
                    return Err(malformed(format!(
                        "residue codebook {b} has no values to read vectors from"
                    )));
                }
                *slot = Some(b);
            }
        }
        books.push(row);
    }
    let classbook = exists(classbook)?;
    if begin > end {
        return Err(malformed(format!(
            "residue begins after it ends ({begin} > {end})"
        )));
    }
    // libvorbis: every combination of classifications in one class book entry must exist.
    let book = &codebooks[usize::from(classbook)];
    let mut partvals = 1u64;
    for _ in 0..book.dims() {
        partvals *= u64::from(classifications);
        if partvals > u64::from(book.entries()) {
            return Err(malformed(format!(
                "residue's {classifications} classifications need more than the {} entries of \
                 its class book",
                book.entries()
            )));
        }
    }
    Ok(Residue {
        kind: kind as u8,
        begin: begin as usize,
        end: end as usize,
        partition_size: partition_size as usize,
        classifications,
        classbook,
        books,
        partvals: partvals as u32,
    })
}

/// Work space for residue decoding, sized for the largest block.
#[derive(Debug, Clone)]
pub(crate) struct Scratch {
    classes: Vec<u8>,
    symbols: Vec<u32>,
    interleaved: Vec<f32>,
}

impl Scratch {
    /// For `channels` channels of up to `half` lines.
    pub(crate) fn new(channels: usize, half: usize) -> Self {
        Self {
            classes: vec![0; channels * half],
            symbols: vec![0; half],
            interleaved: vec![0.0; channels * half],
        }
    }

    pub(crate) fn bytes(channels: usize, half: usize) -> usize {
        channels * half * 5 + half * 4
    }
}

impl Residue {
    /// Decodes this residue into the spectra of the channels in `chans` (each `stride` apart
    /// in `spectra`, `half` lines long, zeroed by the caller), skipping those whose `coded`
    /// flag is clear. The end of the packet (or a class past libvorbis's partvals) ends the
    /// decode; what was read stays.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn decode(
        &self,
        books: &[Codebook],
        r: &mut BitReader<'_>,
        chans: &[usize],
        coded: &[bool],
        spectra: &mut [f32],
        stride: usize,
        half: usize,
        scratch: &mut Scratch,
    ) {
        if self.kind < 2 {
            self.run(books, r, chans.len(), half, spectra, scratch.split(), |j| {
                let ch = chans[j];
                coded[ch].then_some(ch * stride)
            });
            return;
        }
        if !chans.iter().any(|&c| coded[c]) {
            return;
        }
        let n = chans.len();
        let (classes, symbols, big) = (
            &mut scratch.classes,
            &mut scratch.symbols,
            &mut scratch.interleaved[..n * half],
        );
        big.fill(0.0);
        self.run(books, r, 1, n * half, big, (classes, symbols), |_| Some(0));
        if n == 2 {
            let (a, b) = (chans[0] * stride, chans[1] * stride);
            for (i, pair) in big.as_chunks::<2>().0.iter().enumerate() {
                spectra[a + i] = pair[0];
                spectra[b + i] = pair[1];
            }
        } else {
            for (j, &c) in chans.iter().enumerate() {
                let dst = &mut spectra[c * stride..c * stride + half];
                for (d, s) in dst.iter_mut().zip(big[j..].iter().step_by(n)) {
                    *d = *s;
                }
            }
        }
    }

    /// The decode of section 8.6.2 over `vectors` vectors of `len` lines, vector `j` at
    /// `offset(j)` in `buf` (`None`: not coded), type 2 running as type 1 on one vector.
    #[allow(clippy::too_many_arguments)]
    fn run(
        &self,
        books: &[Codebook],
        r: &mut BitReader<'_>,
        vectors: usize,
        len: usize,
        buf: &mut [f32],
        (classes, symbols): (&mut Vec<u8>, &mut Vec<u32>),
        offset: impl Fn(usize) -> Option<usize>,
    ) {
        let begin = self.begin.min(len);
        let end = self.end.min(len);
        let psize = self.partition_size;
        let parts = (end - begin) / psize;
        if parts == 0 {
            return;
        }
        let classbook = &books[usize::from(self.classbook)];
        let per_word = classbook.dims();
        let c = self.classifications;
        let type0 = self.kind == 0;
        for pass in 0..8 {
            if self.books.iter().all(|row| row[pass].is_none()) && pass > 0 {
                continue;
            }
            let mut p = 0;
            while p < parts {
                if pass == 0 {
                    for j in 0..vectors {
                        if offset(j).is_none() {
                            continue;
                        }
                        let Some(mut t) = classbook.decode_entry(r) else {
                            return;
                        };
                        if t >= self.partvals {
                            return;
                        }
                        let keep = per_word.min(parts - p);
                        let mut drop = per_word - keep;
                        while drop > 0 && t != 0 {
                            t /= c;
                            drop -= 1;
                        }
                        let row = &mut classes[j * parts + p..j * parts + p + keep];
                        for slot in row.iter_mut().rev() {
                            *slot = (t % c) as u8;
                            t /= c;
                        }
                    }
                }
                for _ in 0..per_word {
                    if p >= parts {
                        break;
                    }
                    for j in 0..vectors {
                        let Some(base) = offset(j) else {
                            continue;
                        };
                        let class = usize::from(classes[j * parts + p]);
                        let Some(b) = self.books[class][pass] else {
                            continue;
                        };
                        let at = base + begin + p * psize;
                        let v = &mut buf[at..at + psize];
                        let book = &books[usize::from(b)];
                        let ok = if type0 {
                            partition_type0(book, r, v, symbols)
                        } else {
                            partition_type1(book, r, v)
                        };
                        if !ok {
                            return;
                        }
                    }
                    p += 1;
                }
            }
        }
    }
}

impl Scratch {
    fn split(&mut self) -> (&mut Vec<u8>, &mut Vec<u32>) {
        (&mut self.classes, &mut self.symbols)
    }
}

/// Interleaved vectors (section 8.6.3): all of a partition's entries are read before any is
/// added, as libvorbis does, so a partition cut off by the end of the packet adds nothing.
fn partition_type0(
    book: &Codebook,
    r: &mut BitReader<'_>,
    v: &mut [f32],
    symbols: &mut [u32],
) -> bool {
    if !book.has_used_entries() {
        return true;
    }
    let dims = book.dims();
    let step = v.len() / dims;
    if step == 0 {
        return true;
    }
    let symbols = &mut symbols[..step];
    for s in symbols.iter_mut() {
        match book.decode_symbol(r) {
            Some(x) => *s = x,
            None => return false,
        }
    }
    let vq = book.vq();
    for (k, lane) in v.chunks_exact_mut(step).take(dims).enumerate() {
        for (x, &s) in lane.iter_mut().zip(symbols.iter()) {
            *x += vq[s as usize * dims + k];
        }
    }
    true
}

/// Consecutive vectors (section 8.6.4); the last one is cut at the end of the partition and
/// entries read before the end of the packet stay, as libvorbis does.
fn partition_type1(book: &Codebook, r: &mut BitReader<'_>, v: &mut [f32]) -> bool {
    if !book.has_used_entries() {
        return true;
    }
    match book.dims() {
        1 => rows::<1>(book, r, v),
        2 => rows::<2>(book, r, v),
        4 => rows::<4>(book, r, v),
        8 => rows::<8>(book, r, v),
        _ => rows_any(book, r, v),
    }
}

/// Whole vectors of a book of `D` dimensions, then any part-vector at the end.
#[inline(always)]
fn rows<const D: usize>(book: &Codebook, r: &mut BitReader<'_>, v: &mut [f32]) -> bool {
    let vq = book.vq();
    let (whole, rest) = v.as_chunks_mut::<D>();
    for x in whole {
        let Some(s) = book.decode_symbol(r) else {
            return false;
        };
        let y = &vq[s as usize * D..][..D];
        for k in 0..D {
            x[k] += y[k];
        }
    }
    rest.is_empty() || rows_any(book, r, rest)
}

fn rows_any(book: &Codebook, r: &mut BitReader<'_>, v: &mut [f32]) -> bool {
    let dims = book.dims();
    let vq = book.vq();
    let mut i = 0;
    while i < v.len() {
        let Some(s) = book.decode_symbol(r) else {
            return false;
        };
        let row = &vq[s as usize * dims..s as usize * dims + dims];
        let take = dims.min(v.len() - i);
        for (x, &y) in v[i..i + take].iter_mut().zip(row) {
            *x += y;
        }
        i += take;
    }
    true
}
