#![no_main]
//! Vorbis headers and audio packets never panic, hang or allocate past the budget; every
//! sample out is finite and bounded; and a reset decoder decodes the same packets to the same
//! bits. Half the inputs are raw (headers and packets taken whole from the input, each behind
//! a two-byte length), half are built: a setup made by the test writer from fields the input
//! chooses (valid or not), so that the fuzzer reaches the audio decode without first having to
//! find a working setup header.

use arbitrary::{Arbitrary, Result, Unstructured};
use crossfade_fuzz::limits;
use crossfade_vorbis::writer::{
    self as w, Book, Floor0, Floor1, Floor1Class, Lookup, Mapping, Mode, Residue,
};
use crossfade_vorbis::{Comments, Decoder, Ident, Setup};
use libfuzzer_sys::fuzz_target;

/// Every sample out lies within this (crossfade-vorbis's output limit).
const LIMIT: f32 = 16.0;

fn take<'a>(u: &mut Unstructured<'a>) -> Result<&'a [u8]> {
    let n = u16::arbitrary(u)? as usize;
    u.bytes(n.min(u.len()))
}

fn small(u: &mut Unstructured<'_>, max: u32) -> Result<u32> {
    u.int_in_range(0..=max)
}

fn book(u: &mut Unstructured<'_>) -> Result<Book> {
    let bits = small(u, 6)? as u8;
    let dims = 1 + small(u, 4)? as u16;
    let mut lengths = vec![bits.max(1); 1 << bits];
    let ordered = bool::arbitrary(u)?;
    // Sometimes an unused entry (the tree is then refused, which is worth reaching too).
    if !ordered && lengths.len() > 2 && bool::arbitrary(u)? {
        let at = small(u, lengths.len() as u32 - 1)? as usize;
        lengths[at] = 0;
    }
    let mut b = Book {
        dims,
        lengths,
        ordered,
        lookup: Lookup::None,
    };
    if bool::arbitrary(u)? {
        let kind = 1 + small(u, 1)? as u8;
        let entries = b.lengths.len() as u32;
        let count = if kind == 1 {
            (1..=entries)
                .rev()
                .find(|r| r.pow(u32::from(dims)) <= entries)
                .unwrap_or(0)
        } else {
            entries * u32::from(dims)
        };
        let value_bits = 1 + small(u, 7)? as u8;
        let mut multiplicands = Vec::with_capacity(count as usize);
        for _ in 0..count {
            multiplicands.push(small(u, (1 << value_bits) - 1)?);
        }
        b.lookup = Lookup::Values {
            kind,
            minimum: u32::arbitrary(u)? & 0x80FF_FFFF | 0x6000_0000,
            delta: u32::arbitrary(u)? & 0x00FF_FFFF | 0x6000_0000,
            value_bits,
            sequence: bool::arbitrary(u)?,
            multiplicands,
        };
    }
    Ok(b)
}

fn built(u: &mut Unstructured<'_>) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>, w::Setup, u8)> {
    let channels = 1 + small(u, 3)? as u8;
    let e0 = 6 + small(u, 3)?;
    let e1 = e0 + small(u, 2)?;
    let ident = w::ident(channels, 8000 + small(u, 40_000)?, [1 << e0, 1 << e1]);
    let nbooks = 1 + small(u, 4)? as usize;
    let books = (0..nbooks).map(|_| book(u)).collect::<Result<Vec<_>>>()?;
    let nb = nbooks as u32 - 1;
    let floor = |u: &mut Unstructured<'_>| -> Result<w::Floor> {
        Ok(if bool::arbitrary(u)? {
            w::Floor::Zero(Floor0 {
                order: 1 + small(u, 20)? as u8,
                rate: 1 + small(u, 48_000)? as u16,
                bark_map_size: 1 + small(u, 600)? as u16,
                amplitude_bits: small(u, 63)? as u8,
                amplitude_offset: small(u, 255)? as u8,
                books: (0..1 + small(u, 2)?)
                    .map(|_| small(u, nb).map(|b| b as u8))
                    .collect::<Result<_>>()?,
            })
        } else {
            let nclasses = 1 + small(u, 2)? as usize;
            let classes = (0..nclasses)
                .map(|_| {
                    let sub = small(u, 2)? as u8;
                    Ok(Floor1Class {
                        dims: 1 + small(u, 3)? as u8,
                        subclass_bits: sub,
                        masterbook: small(u, nb)? as u8,
                        subbooks: (0..1 << sub)
                            .map(|_| Ok(small(u, nb + 1)? as i16 - 1))
                            .collect::<Result<_>>()?,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let partition_classes: Vec<u8> = (0..small(u, 4)?)
                .map(|_| small(u, nclasses as u32 - 1).map(|c| c as u8))
                .collect::<Result<_>>()?;
            let range_bits = 4 + small(u, 8)? as u8;
            let posts: u32 = partition_classes
                .iter()
                .map(|&c| u32::from(classes[usize::from(c)].dims))
                .sum();
            let mut xs = vec![];
            while (xs.len() as u32) < posts {
                xs.push(1 + small(u, (1 << range_bits) - 2)?);
            }
            w::Floor::One(Floor1 {
                partition_classes,
                classes,
                multiplier: 1 + small(u, 3)? as u8,
                range_bits,
                xs,
            })
        })
    };
    let floors = (0..1 + small(u, 1)?)
        .map(|_| floor(u))
        .collect::<Result<Vec<_>>>()?;
    let residue = |u: &mut Unstructured<'_>| -> Result<Residue> {
        let classes = 1 + small(u, 3)? as usize;
        Ok(Residue {
            kind: small(u, 2)? as u16,
            begin: small(u, 64)?,
            end: small(u, 2048)?,
            partition_size: 1 + small(u, 63)?,
            classbook: small(u, nb)? as u8,
            books: (0..classes)
                .map(|_| {
                    let mut row = [None; 8];
                    for slot in &mut row {
                        if u.ratio(1, 3)? {
                            *slot = Some(small(u, nb)? as u8);
                        }
                    }
                    Ok(row)
                })
                .collect::<Result<_>>()?,
        })
    };
    let residues = (0..1 + small(u, 1)?)
        .map(|_| residue(u))
        .collect::<Result<Vec<_>>>()?;
    let submaps = 1 + small(u, 1)? as usize;
    let mapping = Mapping {
        submaps: (0..submaps)
            .map(|_| {
                Ok((
                    small(u, floors.len() as u32 - 1)? as u8,
                    small(u, residues.len() as u32 - 1)? as u8,
                ))
            })
            .collect::<Result<_>>()?,
        coupling: if channels > 1 && bool::arbitrary(u)? {
            vec![(0, 1)]
        } else {
            vec![]
        },
        mux: (0..channels)
            .map(|_| small(u, submaps as u32 - 1).map(|m| m as u8))
            .collect::<Result<_>>()?,
    };
    let modes = vec![
        Mode {
            long: false,
            mapping: 0,
        },
        Mode {
            long: true,
            mapping: 0,
        },
    ];
    let setup = w::Setup {
        books,
        floors,
        residues,
        mappings: vec![mapping],
        modes,
    };
    let bytes = setup.write(channels);
    Ok((ident, w::comment("fuzz", &[]), bytes, setup, channels))
}

fn decode_all(ident: &Ident, setup: &Setup, packets: &[&[u8]]) -> Vec<Vec<u32>> {
    let mut d = Decoder::new(ident, setup.clone()).expect("headers that parsed make a decoder");
    let mut runs = vec![];
    for _ in 0..2 {
        let mut out = vec![];
        for p in packets {
            let before = out.len();
            match d.decode(p, &mut out) {
                Ok(n) => assert_eq!(out.len(), before + n * usize::from(ident.channels)),
                Err(_) => assert_eq!(out.len(), before),
            }
        }
        assert!(out.iter().all(|v| v.is_finite() && v.abs() <= LIMIT));
        runs.push(out.iter().map(|v| v.to_bits()).collect());
        d.reset();
    }
    runs
}

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let Ok(raw) = bool::arbitrary(&mut u) else {
        return;
    };
    let (ident, comment, setup, built_setup) = if raw {
        let (Ok(a), Ok(b), Ok(c)) = (take(&mut u), take(&mut u), take(&mut u)) else {
            return;
        };
        (a.to_vec(), b.to_vec(), c.to_vec(), None)
    } else {
        let Ok((a, b, c, s, _)) = built(&mut u) else {
            return;
        };
        (a, b, c, Some(s))
    };
    let mut packets = vec![];
    while let Ok(p) = take(&mut u) {
        if p.is_empty() && u.is_empty() {
            break;
        }
        // A built stream's packets get a valid start, so that most reach the floors.
        let p = match &built_setup {
            Some(s) if !p.is_empty() => {
                let mut wr = s.start_packet(p[0] & 1, p[0] & 2 != 0, p[0] & 4 != 0);
                wr.put_bytes(&p[1..]);
                wr.into_bytes()
            }
            _ => p.to_vec(),
        };
        packets.push(p);
        if packets.len() == 64 {
            break;
        }
    }
    let budget = limits().max_alloc_bytes;
    let _ = Comments::parse(&comment, budget);
    let _ = crossfade_vorbis::unpack_headers(data);
    let Ok(id) = Ident::parse(&ident) else { return };
    let Ok(s) = Setup::parse(&setup, &id, budget) else {
        return;
    };
    assert!(s.heap_bytes() <= budget);
    for p in &packets {
        let _ = s.modes().block_size(p);
    }
    let refs: Vec<&[u8]> = packets.iter().map(Vec::as_slice).collect();
    let runs = decode_all(&id, &s, &refs);
    // After a reset, the first packet yields nothing; what follows must match.
    assert_eq!(runs[0], runs[1], "a reset decoder decoded differently");
});
