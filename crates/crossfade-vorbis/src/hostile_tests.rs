//! Hostile input: mutations of constructed streams never panic, never make a setup hold more
//! than its budget, and never make the decoder give a sample that is not finite and bounded.

use crate::decoder::OUTPUT_LIMIT;
use crate::testing::{self as w, Book, Floor0, Floor1, Floor1Class, Mapping, Mode, Residue};
use crate::{Comments, Decoder, Ident, Setup};
use proptest::prelude::*;

/// Two channels, coupled, every floor and residue type, four modes.
fn setup() -> w::Setup {
    let vq = |bits: u8, dims: u16| {
        let entries = 1u32 << bits;
        Book {
            dims,
            ..Book::flat(bits)
        }
        .with_values(
            -3,
            &(0..entries * u32::from(dims))
                .map(|v| v % 7)
                .collect::<Vec<_>>(),
        )
    };
    w::Setup {
        books: vec![vq(3, 2), Book::flat(2), Book::flat(5), vq(4, 1), vq(2, 4)],
        floors: vec![
            w::Floor::One(Floor1 {
                partition_classes: vec![0, 1, 0],
                classes: vec![
                    Floor1Class {
                        dims: 3,
                        subclass_bits: 1,
                        masterbook: 1,
                        subbooks: vec![2, -1],
                    },
                    Floor1Class {
                        dims: 2,
                        subclass_bits: 0,
                        masterbook: 0,
                        subbooks: vec![2],
                    },
                ],
                multiplier: 2,
                range_bits: 7,
                xs: vec![50, 10, 90, 30, 70, 5, 110, 60],
            }),
            w::Floor::Zero(Floor0 {
                order: 6,
                rate: 44_100,
                bark_map_size: 128,
                amplitude_bits: 6,
                amplitude_offset: 40,
                books: vec![0, 4],
            }),
        ],
        residues: (0..3)
            .map(|kind| Residue {
                kind,
                begin: 4,
                end: 200,
                partition_size: 8,
                classbook: 1,
                books: vec![
                    [Some(0), None, Some(3), None, None, None, None, None],
                    [None, Some(4), None, None, None, None, None, Some(0)],
                    [None; 8],
                    [Some(3), Some(3), None, None, None, None, None, None],
                ],
            })
            .collect(),
        mappings: vec![
            Mapping {
                submaps: vec![(0, 2)],
                coupling: vec![(0, 1)],
                mux: vec![],
            },
            Mapping {
                submaps: vec![(1, 0), (0, 1)],
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
                mapping: 1,
            },
            Mode {
                long: false,
                mapping: 1,
            },
        ],
    }
}

fn mutate(bytes: &mut Vec<u8>, edits: &[(usize, u8)], cut: Option<usize>) {
    if bytes.is_empty() {
        return;
    }
    for &(at, x) in edits {
        let i = at % bytes.len();
        bytes[i] ^= x;
    }
    if let Some(c) = cut {
        bytes.truncate(c % (bytes.len() + 1));
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn mutated_streams_never_panic_or_exceed_their_budget(
        target in 0usize..4,
        edits in prop::collection::vec((any::<usize>(), 1u8..=255), 0..6),
        cut in prop::option::of(any::<usize>()),
        budget in prop_oneof![Just(usize::MAX), 0usize..400_000],
        payloads in prop::collection::vec(
            (0u8..4, prop::collection::vec(any::<u8>(), 0..120)), 1..8),
    ) {
        let s = setup();
        let mut ident = w::ident(2, 44_100, [256, 512]);
        let mut comment = w::comment("vendor", &["A=b", "TITLE=c"]);
        let mut setup_bytes = s.write(2);
        let mut packets: Vec<Vec<u8>> = payloads
            .iter()
            .map(|(mode, body)| {
                let mut wr = s.start_packet(mode % 3, true, mode & 1 == 0);
                wr.put_bytes(body);
                wr.into_bytes()
            })
            .collect();
        match target {
            0 => mutate(&mut ident, &edits, cut),
            1 => mutate(&mut comment, &edits, cut),
            2 => mutate(&mut setup_bytes, &edits, cut),
            _ => {
                let k = edits.first().map_or(0, |e| e.0 % packets.len());
                mutate(&mut packets[k], &edits, cut);
            }
        }
        let _ = Comments::parse(&comment, 1000);
        let Ok(id) = Ident::parse(&ident) else { return Ok(()) };
        let Ok(parsed) = Setup::parse(&setup_bytes, &id, budget) else { return Ok(()) };
        prop_assert!(parsed.heap_bytes() <= budget);
        prop_assert!(parsed.held_bytes() <= parsed.heap_bytes(),
            "holds {} but was charged {}", parsed.held_bytes(), parsed.heap_bytes());
        let channels = usize::from(id.channels);
        let mut d = Decoder::new(&id, parsed).unwrap();
        let mut out = vec![];
        for p in &packets {
            let before = out.len();
            if let Ok(n) = d.decode(p, &mut out) {
                prop_assert_eq!(out.len(), before + n * channels);
            } else {
                prop_assert_eq!(out.len(), before);
            }
        }
        prop_assert!(out.iter().all(|v| v.is_finite() && v.abs() <= OUTPUT_LIMIT));
    }

    #[test]
    fn random_bytes_as_any_header_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..400)) {
        let id = Ident::parse(&w::ident(3, 8000, [64, 8192])).unwrap();
        let mut setup = vec![5];
        setup.extend_from_slice(b"vorbis");
        setup.extend_from_slice(&bytes);
        if let Ok(s) = Setup::parse(&setup, &id, 1 << 20) {
            prop_assert!(s.held_bytes() <= s.heap_bytes());
        }
        let mut comment = vec![3];
        comment.extend_from_slice(b"vorbis");
        comment.extend_from_slice(&bytes);
        let _ = Comments::parse(&comment, 1 << 10);
        let _ = Ident::parse(&bytes);
        let _ = crate::unpack_headers(&bytes);
    }
}

#[test]
fn the_stream_the_mutations_start_from_is_valid() {
    let s = setup();
    let id = Ident::parse(&w::ident(2, 44_100, [256, 512])).unwrap();
    let parsed = Setup::parse(&s.write(2), &id, usize::MAX).unwrap();
    let mut d = Decoder::new(&id, parsed).unwrap();
    let mut out = vec![];
    for mode in [0u8, 1, 2, 1, 0] {
        let mut wr = s.start_packet(mode, true, true);
        wr.put_bytes(&[0xA5; 60]);
        d.decode(&wr.into_bytes(), &mut out).unwrap();
    }
    assert!(out.iter().any(|&v| v != 0.0));
}
