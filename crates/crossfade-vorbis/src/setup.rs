//! The setup header (specification section 4.2.4): codebooks, floors, residues, mappings and
//! modes, every index checked here so that decoding needs no check that can fail.

use crate::bits::{BitReader, ilog};
use crate::codebook::{Budget, Codebook};
use crate::error::{Error, Result, malformed};
use crate::floor::{self, Floor};
use crate::header::{HeaderKind, Ident, header_kind};
use crate::residue::{self, Residue};

#[derive(Debug, Clone)]
pub(crate) struct Mapping {
    /// (magnitude, angle) channel pairs, in the order they were coded.
    pub(crate) coupling: Vec<(u8, u8)>,
    /// The submap of each channel.
    pub(crate) mux: Vec<u8>,
    /// (floor, residue) of each submap.
    pub(crate) submaps: Vec<(u8, u8)>,
}

/// What a container needs to time packets: each mode's block size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Modes {
    block_sizes: [u16; 2],
    long: Vec<bool>,
}

impl Modes {
    pub fn count(&self) -> usize {
        self.long.len()
    }

    /// Whether `mode` uses the long block (false for a mode that does not exist).
    pub fn is_long(&self, mode: usize) -> bool {
        self.long.get(mode).copied().unwrap_or(false)
    }

    /// The short and the long block size, in samples.
    pub fn block_sizes(&self) -> [u16; 2] {
        self.block_sizes
    }

    /// Reads an audio packet's type bit and mode number.
    pub(crate) fn read_mode(&self, r: &mut BitReader<'_>) -> Result<usize> {
        match r.read_bit() {
            None => return Err(Error::BadPacket("empty packet")),
            Some(true) => return Err(Error::BadPacket("a header where audio was expected")),
            Some(false) => {}
        }
        let mode = r
            .read(ilog(self.long.len() as u32 - 1))
            .ok_or(Error::BadPacket("packet ends before its mode"))? as usize;
        if mode >= self.long.len() {
            return Err(Error::BadPacket("mode does not exist"));
        }
        Ok(mode)
    }

    /// The block size of an audio packet, from its first bits, without decoding it. An empty
    /// packet, a header, or a mode that does not exist is an error, never a panic.
    pub fn block_size(&self, packet: &[u8]) -> Result<usize> {
        let mode = self.read_mode(&mut BitReader::new(packet))?;
        Ok(usize::from(self.block_sizes[usize::from(self.long[mode])]))
    }
}

/// The sample frames a packet of block size `block` yields after one of `previous_block`: from
/// the centre of the previous window to the centre of this one. The first packet after the
/// headers (or a reset) yields none.
pub fn packet_frames(previous_block: usize, block: usize) -> usize {
    previous_block / 4 + block / 4
}

/// The parsed setup header.
#[derive(Debug, Clone)]
pub struct Setup {
    pub(crate) ident: Ident,
    pub(crate) codebooks: Vec<Codebook>,
    pub(crate) floors: Vec<Floor>,
    pub(crate) residues: Vec<Residue>,
    pub(crate) mappings: Vec<Mapping>,
    /// The mapping of each mode.
    pub(crate) mode_mappings: Vec<u8>,
    modes: Modes,
    charged: usize,
}

impl Setup {
    /// Parses the setup header of a stream with this identification header, allocating no
    /// more than `budget` bytes in all (each table is checked before it is made).
    pub fn parse(packet: &[u8], ident: &Ident, budget: usize) -> Result<Self> {
        if header_kind(packet) != Some(HeaderKind::Setup) {
            return Err(malformed("not a setup header"));
        }
        let mut budget = Budget::new(budget);
        let mut r = BitReader::new(&packet[7..]);
        let eof = || malformed("setup header too short");
        let channels = u32::from(ident.channels);

        let count = r.read(8).ok_or_else(eof)? as usize + 1;
        let mut codebooks = budget.with_capacity(count)?;
        for _ in 0..count {
            codebooks.push(Codebook::parse(&mut r, &mut budget)?);
        }

        let count = r.read(6).ok_or_else(eof)? + 1;
        for _ in 0..count {
            let t = r.read(16).ok_or_else(eof)?;
            if t != 0 {
                return Err(malformed(format!("time domain transform type {t}")));
            }
        }

        let count = r.read(6).ok_or_else(eof)? as usize + 1;
        let mut floors = budget.with_capacity(count)?;
        for _ in 0..count {
            floors.push(floor::parse(
                &mut r,
                &codebooks,
                ident.block_sizes,
                &mut budget,
            )?);
        }

        let count = r.read(6).ok_or_else(eof)? as usize + 1;
        let mut residues = budget.with_capacity(count)?;
        for _ in 0..count {
            residues.push(residue::parse(&mut r, &codebooks, &mut budget)?);
        }

        let count = r.read(6).ok_or_else(eof)? as usize + 1;
        let mut mappings = budget.with_capacity(count)?;
        for _ in 0..count {
            let t = r.read(16).ok_or_else(eof)?;
            if t != 0 {
                return Err(malformed(format!("mapping type {t}")));
            }
            let submap_count = if r.read_bit().ok_or_else(eof)? {
                r.read(4).ok_or_else(eof)? + 1
            } else {
                1
            };
            let mut coupling = Vec::new();
            if r.read_bit().ok_or_else(eof)? {
                let steps = r.read(8).ok_or_else(eof)? as usize + 1;
                coupling = budget.with_capacity(steps)?;
                let bits = ilog(channels - 1);
                for _ in 0..steps {
                    let m = r.read(bits).ok_or_else(eof)?;
                    let a = r.read(bits).ok_or_else(eof)?;
                    if m == a {
                        return Err(malformed(format!(
                            "mapping would couple channel {m} with itself"
                        )));
                    }
                    if let Some(c) = [m, a].into_iter().find(|&c| c >= channels) {
                        return Err(malformed(format!(
                            "mapping couples channel {c}, which does not exist"
                        )));
                    }
                    coupling.push((m as u8, a as u8));
                }
            }
            if r.read(2).ok_or_else(eof)? != 0 {
                return Err(malformed("mapping reserved field is not zero"));
            }
            let mut mux = budget.vec(channels as usize, 0u8)?;
            if submap_count > 1 {
                for m in &mut mux {
                    let s = r.read(4).ok_or_else(eof)?;
                    if s >= submap_count {
                        return Err(malformed(format!(
                            "mapping uses submap {s}, which does not exist"
                        )));
                    }
                    *m = s as u8;
                }
            }
            let mut submaps = budget.with_capacity(submap_count as usize)?;
            for _ in 0..submap_count {
                r.read(8).ok_or_else(eof)?;
                let f = r.read(8).ok_or_else(eof)?;
                if f as usize >= floors.len() {
                    return Err(malformed(format!(
                        "mapping uses floor {f}, which does not exist"
                    )));
                }
                let res = r.read(8).ok_or_else(eof)?;
                if res as usize >= residues.len() {
                    return Err(malformed(format!(
                        "mapping uses residue {res}, which does not exist"
                    )));
                }
                submaps.push((f as u8, res as u8));
            }
            mappings.push(Mapping {
                coupling,
                mux,
                submaps,
            });
        }

        let count = r.read(6).ok_or_else(eof)? as usize + 1;
        let mut long = budget.with_capacity(count)?;
        let mut mode_mappings = budget.with_capacity(count)?;
        for _ in 0..count {
            long.push(r.read_bit().ok_or_else(eof)?);
            let window = r.read(16).ok_or_else(eof)?;
            if window != 0 {
                return Err(malformed(format!("mode window type {window}")));
            }
            let transform = r.read(16).ok_or_else(eof)?;
            if transform != 0 {
                return Err(malformed(format!("mode transform type {transform}")));
            }
            let m = r.read(8).ok_or_else(eof)?;
            if m as usize >= mappings.len() {
                return Err(malformed(format!(
                    "mode uses mapping {m}, which does not exist"
                )));
            }
            mode_mappings.push(m as u8);
        }
        if r.read_bit() != Some(true) {
            return Err(malformed("setup header framing bit not set"));
        }
        Ok(Self {
            ident: *ident,
            codebooks,
            floors,
            residues,
            mappings,
            mode_mappings,
            modes: Modes {
                block_sizes: ident.block_sizes,
                long,
            },
            charged: budget.used(),
        })
    }

    /// The modes, for timing packets.
    pub fn modes(&self) -> &Modes {
        &self.modes
    }

    /// The identification header this setup was parsed with.
    pub fn ident(&self) -> &Ident {
        &self.ident
    }

    /// The bytes the setup was charged against the budget: what it holds, and the tables it
    /// built them from.
    pub fn heap_bytes(&self) -> usize {
        self.charged
    }

    /// The bytes it actually holds on the heap.
    #[cfg(test)]
    pub(crate) fn held_bytes(&self) -> usize {
        self.codebooks
            .iter()
            .map(Codebook::heap_bytes)
            .sum::<usize>()
            + self.codebooks.capacity() * size_of::<Codebook>()
            + self.floors.iter().map(Floor::heap_bytes).sum::<usize>()
            + self.floors.capacity() * size_of::<Floor>()
            + self
                .residues
                .iter()
                .map(|r| r.books.capacity() * size_of::<[Option<u8>; 8]>())
                .sum::<usize>()
            + self.residues.capacity() * size_of::<Residue>()
            + self
                .mappings
                .iter()
                .map(|m| m.coupling.capacity() * 2 + m.mux.capacity() + m.submaps.capacity() * 2)
                .sum::<usize>()
            + self.mappings.capacity() * size_of::<Mapping>()
            + self.modes.long.capacity()
            + self.mode_mappings.capacity()
    }
}

#[cfg(test)]
#[path = "setup_tests.rs"]
mod tests;
