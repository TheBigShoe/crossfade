use super::*;
use crate::testing::{
    self as w, BitWriter, Book, Floor0, Floor1, Floor1Class, Mapping, Mode, Residue,
};

fn ident(channels: u8) -> Ident {
    Ident::parse(&w::ident(channels, 44_100, [256, 2048])).unwrap()
}

/// A setup that uses every kind of section: both floor types, a residue of each type, two
/// mappings (one with coupling and two submaps) and four modes.
fn rich() -> w::Setup {
    w::Setup {
        books: vec![
            Book {
                dims: 2,
                ..Book::flat(2)
            }
            .with_values(-1, &[0, 0, 0, 1, 1, 0, 1, 1]),
            Book::scalar(vec![1]),
            Book::flat(3),
            Book {
                dims: 2,
                ..Book::flat(2)
            }
            .with_values(0, &[1, 2, 3, 4, 5, 6, 7, 8]),
        ],
        floors: vec![
            w::Floor::One(Floor1 {
                partition_classes: vec![0, 0],
                classes: vec![Floor1Class {
                    dims: 2,
                    subclass_bits: 0,
                    masterbook: 0,
                    subbooks: vec![2],
                }],
                multiplier: 2,
                range_bits: 7,
                xs: vec![64, 32, 96, 16],
            }),
            w::Floor::Zero(Floor0 {
                order: 4,
                rate: 44_100,
                bark_map_size: 256,
                amplitude_bits: 6,
                amplitude_offset: 100,
                books: vec![3],
            }),
        ],
        residues: (0..3)
            .map(|kind| Residue {
                kind,
                begin: 0,
                end: 256,
                partition_size: 16,
                classbook: 1,
                books: vec![[Some(0), None, Some(0), None, None, None, None, None]],
            })
            .collect(),
        mappings: vec![
            Mapping {
                submaps: vec![(0, 2)],
                coupling: vec![(0, 1)],
                mux: vec![],
            },
            Mapping {
                submaps: vec![(0, 0), (1, 1)],
                coupling: vec![],
                mux: vec![1, 0],
            },
        ],
        modes: vec![
            Mode {
                long: false,
                mapping: 0,
            },
            Mode {
                long: true,
                mapping: 0,
            },
            Mode {
                long: false,
                mapping: 1,
            },
            Mode {
                long: true,
                mapping: 1,
            },
        ],
    }
}

#[derive(Clone, Copy)]
struct Tweaks {
    kind: u8,
    time_value: u16,
    mapping_type: u16,
    reserved: u8,
    window: u16,
    transform: u16,
    framing: bool,
}

const AS_WRITTEN: Tweaks = Tweaks {
    kind: 5,
    time_value: 0,
    mapping_type: 0,
    reserved: 0,
    window: 0,
    transform: 0,
    framing: true,
};

/// `w::Setup::write` with fields the writer always gets right made wrong.
fn assemble(s: &w::Setup, channels: u8, t: Tweaks) -> Vec<u8> {
    let mut b = BitWriter::new();
    b.put(u64::from(t.kind), 8);
    b.put_bytes(b"vorbis");
    b.put(s.books.len() as u64 - 1, 8);
    for book in &s.books {
        book.write(&mut b);
    }
    b.put(0, 6);
    b.put(u64::from(t.time_value), 16);
    b.put(s.floors.len() as u64 - 1, 6);
    for f in &s.floors {
        f.write(&mut b);
    }
    b.put(s.residues.len() as u64 - 1, 6);
    for r in &s.residues {
        r.write(&mut b);
    }
    b.put(s.mappings.len() as u64 - 1, 6);
    for m in &s.mappings {
        let mut mb = BitWriter::new();
        m.write(&mut mb, channels);
        let bytes = mb.into_bytes();
        // Replace the type (the first 16 bits), then copy the rest bit by bit.
        b.put(u64::from(t.mapping_type), 16);
        let mut r = BitReader::new(&bytes);
        r.read(16).unwrap();
        let total = m_bits(m, channels) - 16;
        let mut copied = 0;
        // The reserved field follows the submap and coupling fields.
        let before_reserved = 1
            + if m.submaps.len() > 1 { 4 } else { 0 }
            + 1
            + if m.coupling.is_empty() {
                0
            } else {
                8 + m.coupling.len() as u32 * 2 * w::ilog(u32::from(channels) - 1)
            };
        while copied < total {
            if copied == before_reserved {
                r.read(2).unwrap();
                b.put(u64::from(t.reserved), 2);
                copied += 2;
                continue;
            }
            b.put(u64::from(r.read(1).unwrap()), 1);
            copied += 1;
        }
    }
    b.put(s.modes.len() as u64 - 1, 6);
    for m in &s.modes {
        b.put_bit(m.long);
        b.put(u64::from(t.window), 16);
        b.put(u64::from(t.transform), 16);
        b.put(u64::from(m.mapping), 8);
    }
    b.put_bit(t.framing);
    b.into_bytes()
}

fn m_bits(m: &Mapping, channels: u8) -> u32 {
    let mut b = BitWriter::new();
    m.write(&mut b, channels);
    b.bits() as u32
}

fn parse(s: &w::Setup, channels: u8) -> Result<Setup> {
    Setup::parse(&s.write(channels), &ident(channels), usize::MAX)
}

fn refused(s: &w::Setup, channels: u8) -> String {
    match parse(s, channels) {
        Err(Error::Malformed(what)) => what,
        Err(e) => panic!("expected Malformed, got {e:?}"),
        Ok(_) => panic!("expected Malformed, got a setup"),
    }
}

fn refused_tweaked(t: Tweaks) -> String {
    match Setup::parse(&assemble(&rich(), 2, t), &ident(2), usize::MAX) {
        Err(Error::Malformed(what)) => what,
        Err(e) => panic!("expected Malformed, got {e:?}"),
        Ok(_) => panic!("expected Malformed, got a setup"),
    }
}

#[test]
fn the_assembler_writes_what_the_writer_writes() {
    assert_eq!(assemble(&rich(), 2, AS_WRITTEN), rich().write(2));
}

#[test]
fn a_setup_with_every_kind_of_section_is_read() {
    let s = parse(&rich(), 2).unwrap();
    assert_eq!(s.modes().count(), 4);
    assert_eq!(
        (0..4).map(|m| s.modes().is_long(m)).collect::<Vec<_>>(),
        [false, true, false, true]
    );
    assert_eq!(s.codebooks.len(), 4);
    assert_eq!(s.floors.len(), 2);
    assert_eq!(s.residues.len(), 3);
    assert_eq!(s.mappings.len(), 2);
}

#[test]
fn the_block_size_of_a_packet_comes_from_its_mode_without_decoding() {
    let s = parse(&rich(), 2).unwrap();
    let m = s.modes();
    // Type bit 0, then two mode bits.
    assert_eq!(m.block_size(&[0b000]), Ok(256));
    assert_eq!(m.block_size(&[0b010]), Ok(2048));
    assert_eq!(m.block_size(&[0b100]), Ok(256));
    assert_eq!(m.block_size(&[0b110]), Ok(2048));
    assert_eq!(packet_frames(256, 2048), 576);
    assert_eq!(packet_frames(2048, 2048), 1024);
    assert_eq!(packet_frames(2048, 256), 576);
    assert!(matches!(m.block_size(&[0b001]), Err(Error::BadPacket(_))));
    assert!(matches!(m.block_size(&[]), Err(Error::BadPacket(_))));
    // Three modes: mode 3 does not exist.
    let mut three = rich();
    three.modes.pop();
    let s = parse(&three, 2).unwrap();
    assert_eq!(s.modes().block_size(&[0b100]), Ok(256));
    assert!(matches!(
        s.modes().block_size(&[0b110]),
        Err(Error::BadPacket(_))
    ));
    // One mode needs no mode bits.
    let mut one = rich();
    one.modes.truncate(1);
    let s = parse(&one, 2).unwrap();
    assert_eq!(s.modes().block_size(&[0b0]), Ok(256));
}

#[test]
fn modes_carry_the_ident_block_sizes() {
    let s = parse(&rich(), 2).unwrap();
    assert_eq!(s.modes().block_sizes(), [256, 2048]);
    let m2 = s.modes().clone();
    assert_eq!(&m2, s.modes());
}

#[test]
fn a_cut_short_setup_is_refused_wherever_it_ends() {
    let bytes = rich().write(2);
    for len in 0..bytes.len() {
        assert!(
            matches!(
                Setup::parse(&bytes[..len], &ident(2), usize::MAX),
                Err(Error::Malformed(_))
            ),
            "{len}"
        );
    }
    assert!(Setup::parse(&bytes, &ident(2), usize::MAX).is_ok());
}

#[test]
fn fields_that_must_be_zero_or_set_are_checked() {
    assert!(
        refused_tweaked(Tweaks {
            kind: 3,
            ..AS_WRITTEN
        })
        .contains("not a setup header")
    );
    assert!(
        refused_tweaked(Tweaks {
            time_value: 1,
            ..AS_WRITTEN
        })
        .contains("time")
    );
    assert!(
        refused_tweaked(Tweaks {
            mapping_type: 1,
            ..AS_WRITTEN
        })
        .contains("mapping type")
    );
    assert!(
        refused_tweaked(Tweaks {
            reserved: 2,
            ..AS_WRITTEN
        })
        .contains("reserved")
    );
    assert!(
        refused_tweaked(Tweaks {
            window: 1,
            ..AS_WRITTEN
        })
        .contains("window")
    );
    assert!(
        refused_tweaked(Tweaks {
            transform: 1,
            ..AS_WRITTEN
        })
        .contains("transform")
    );
    assert!(
        refused_tweaked(Tweaks {
            framing: false,
            ..AS_WRITTEN
        })
        .contains("framing")
    );
}

fn with(f: impl FnOnce(&mut w::Setup)) -> w::Setup {
    let mut s = rich();
    f(&mut s);
    s
}

fn floor0(s: &mut w::Setup) -> &mut Floor0 {
    match &mut s.floors[1] {
        w::Floor::Zero(f) => f,
        w::Floor::One(_) => unreachable!(),
    }
}

fn floor1(s: &mut w::Setup) -> &mut Floor1 {
    match &mut s.floors[0] {
        w::Floor::One(f) => f,
        w::Floor::Zero(_) => unreachable!(),
    }
}

#[test]
fn degenerate_floor_0_values_are_refused() {
    // The lewton hang: a bark map size of 0 gives NaN and a loop that never ends.
    let e = refused(&with(|s| floor0(s).bark_map_size = 0), 2);
    assert!(e.contains("bark map size"), "{e}");
    assert!(refused(&with(|s| floor0(s).order = 0), 2).contains("order"));
    assert!(refused(&with(|s| floor0(s).rate = 0), 2).contains("rate"));
    assert!(refused(&with(|s| floor0(s).books = vec![4]), 2).contains("codebook 4"));
    // A floor 0 book is read as vectors, so it needs values.
    assert!(refused(&with(|s| floor0(s).books = vec![2]), 2).contains("no values"));
}

#[test]
fn bad_floor_1_values_are_refused() {
    assert!(refused(&with(|s| floor1(s).classes[0].subbooks = vec![9]), 2).contains("codebook 9"));
    let e = refused(
        &with(|s| {
            let c = &mut floor1(s).classes[0];
            c.subclass_bits = 1;
            c.masterbook = 7;
            c.subbooks = vec![2, -1];
        }),
        2,
    );
    assert!(e.contains("codebook 7"), "{e}");
    assert!(refused(&with(|s| floor1(s).xs[1] = 64), 2).contains("twice"));
    assert!(refused(&with(|s| floor1(s).xs[1] = 0), 2).contains("twice"));
    // 2 + 8 partitions of 8 posts = 66 posts, one more than Vorbis allows.
    let e = refused(
        &with(|s| {
            let f = floor1(s);
            f.classes[0].dims = 8;
            f.partition_classes = vec![0; 8];
            f.xs = (1..=64).collect();
        }),
        2,
    );
    assert!(e.contains("posts"), "{e}");
    let mut most = with(|s| {
        let f = floor1(s);
        f.classes[0].dims = 7;
        f.partition_classes = vec![0; 9];
        f.xs = (1..=63).collect();
    });
    assert!(parse(&most, 2).is_ok());
    floor1(&mut most).classes[0].dims = 1;
    floor1(&mut most).xs.truncate(9);
    assert!(parse(&most, 2).is_ok());
}

#[test]
fn an_unknown_floor_or_residue_type_is_refused() {
    let mut s = rich();
    s.residues[0].kind = 3;
    assert!(refused(&s, 2).contains("residue type 3"));
    // Floor type 2: patch the written type of the first floor.
    let mut b = BitWriter::new();
    let r = rich();
    b.put(5, 8);
    b.put_bytes(b"vorbis");
    b.put(r.books.len() as u64 - 1, 8);
    for book in &r.books {
        book.write(&mut b);
    }
    b.put(0, 6);
    b.put(0, 16);
    b.put(0, 6);
    b.put(2, 16);
    b.put(0, 64);
    let e = Setup::parse(&b.into_bytes(), &ident(2), usize::MAX).err();
    assert_eq!(e, Some(Error::Malformed("floor type 2".into())));
}

#[test]
fn bad_residues_are_refused() {
    let e = refused(
        &with(|s| {
            s.residues[1].begin = 200;
            s.residues[1].end = 100;
        }),
        2,
    );
    assert!(e.contains("begins after it ends"), "{e}");
    assert!(refused(&with(|s| s.residues[0].classbook = 4), 2).contains("codebook 4"));
    assert!(refused(&with(|s| s.residues[2].books[0][5] = Some(200)), 2).contains("codebook 200"));
    assert!(refused(&with(|s| s.residues[0].books[0][1] = Some(2)), 2).contains("no values"));
    // Two classifications need two entries in the one-entry class book.
    let e = refused(&with(|s| s.residues[0].books.push([None; 8])), 2);
    assert!(e.contains("classifications"), "{e}");
}

#[test]
fn bad_mappings_and_modes_are_refused() {
    assert!(refused(&with(|s| s.mappings[0].coupling = vec![(1, 1)]), 2).contains("couple"));
    let three = |s: &mut w::Setup| {
        s.mappings[0].coupling = vec![(0, 3)];
        s.mappings[1].mux = vec![1, 0, 0];
    };
    assert!(refused(&with(three), 3).contains("channel 3"));
    let four = |s: &mut w::Setup| {
        s.mappings[0].coupling = vec![(0, 3)];
        s.mappings[1].mux = vec![1, 0, 0, 1];
    };
    assert!(parse(&with(four), 4).is_ok());
    assert!(refused(&with(|s| s.mappings[1].mux = vec![2, 0]), 2).contains("submap 2"));
    assert!(refused(&with(|s| s.mappings[1].submaps[1].0 = 2), 2).contains("floor 2"));
    assert!(refused(&with(|s| s.mappings[1].submaps[1].1 = 3), 2).contains("residue 3"));
    assert!(refused(&with(|s| s.modes[2].mapping = 2), 2).contains("mapping 2"));
    // One channel cannot be coupled: its channel numbers have no bits and are both 0.
    let one = |s: &mut w::Setup| {
        s.mappings[0].coupling = vec![(0, 0)];
        s.mappings[1].mux = vec![1];
    };
    assert!(refused(&with(one), 1).contains("couple"));
}

#[test]
fn a_codebook_too_large_for_the_budget_is_refused_and_nothing_is_kept_past_it() {
    let bytes = rich().write(2);
    let s = Setup::parse(&bytes, &ident(2), usize::MAX).unwrap();
    let need = s.heap_bytes();
    assert!(need > 0);
    assert!(s.held_bytes() <= need, "{} > {need}", s.held_bytes());
    assert!(Setup::parse(&bytes, &ident(2), need).is_ok());
    assert!(matches!(
        Setup::parse(&bytes, &ident(2), need / 2),
        Err(Error::OverBudget { budget, .. }) if budget == need / 2
    ));
}

#[test]
fn lewtons_terabyte_codebook_is_refused_before_anything_is_allocated() {
    // From the admission report: 65535 dimensions of 2^24 - 1 entries, lookup type 2.
    let mut b = BitWriter::new();
    b.put(5, 8);
    b.put_bytes(b"vorbis");
    b.put(0, 8);
    b.put(0x56_4342, 24);
    b.put(0xFFFF, 16);
    b.put(0xFF_FFFF, 24);
    b.put(1, 1);
    b.put(0, 5);
    b.put(0xFF_FFFF, 24);
    b.put(2, 4);
    b.put(0, 64);
    b.put(15, 4);
    b.put(0, 1);
    let e = Setup::parse(&b.into_bytes(), &ident(2), 1 << 20).err();
    assert!(
        matches!(e, Some(Error::Malformed(ref w)) if w.contains("too large")),
        "{e:?}"
    );
}
