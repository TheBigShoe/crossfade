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
