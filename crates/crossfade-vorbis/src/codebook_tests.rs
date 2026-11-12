use super::*;
use crate::bits::BitReader;
use crate::writer::{BitWriter, Book, Lookup, float32_pack};

fn parse(book: &Book) -> Result<Codebook> {
    parse_with(book, usize::MAX)
}

fn parse_with(book: &Book, budget: usize) -> Result<Codebook> {
    let mut w = BitWriter::new();
    book.write(&mut w);
    let bytes = w.into_bytes();
    Codebook::parse(&mut BitReader::new(&bytes), &mut Budget::new(budget))
}

fn malformed(r: Result<Codebook>) -> String {
    match r {
        Err(Error::Malformed(what)) => what,
        Err(e) => panic!("expected Malformed, got {e:?}"),
        Ok(_) => panic!("expected Malformed, got a codebook"),
    }
}

/// Encodes `entries` with the writer and decodes them back.
fn round_trip(book: &Book, entries: &[u32]) {
    let cb = parse(book).unwrap();
    let mut w = BitWriter::new();
    for &e in entries {
        book.encode(&mut w, e);
    }
    let bits = w.bits();
    let bytes = w.into_bytes();
    let mut r = BitReader::new(&bytes);
    for &e in entries {
        assert_eq!(cb.decode_entry(&mut r), Some(e));
    }
    assert_eq!(r.bits_left(), bytes.len() * 8 - bits);
}

#[test]
fn codewords_are_assigned_as_in_the_specification() {
    // Section 3.2.1's example.
    let book = Book::scalar(vec![2, 4, 4, 4, 4, 2, 3, 3]);
    let words: Vec<_> = book.codewords().into_iter().map(Option::unwrap).collect();
    assert_eq!(
        words,
        [
            (0b00, 2),
            (0b0100, 4),
            (0b0101, 4),
            (0b0110, 4),
            (0b0111, 4),
            (0b10, 2),
            (0b110, 3),
            (0b111, 3)
        ]
    );
    round_trip(&book, &[0, 1, 2, 3, 4, 5, 6, 7, 7, 6, 5, 0, 0, 3]);
}

#[test]
fn flat_sparse_and_ordered_books_decode_every_entry() {
    let flat = Book::flat(5);
    round_trip(&flat, &(0..32).rev().collect::<Vec<_>>());
    let sparse = Book::scalar(vec![0, 2, 0, 0, 2, 3, 0, 3, 2, 0]);
    round_trip(&sparse, &[1, 4, 5, 7, 8, 8, 1]);
    let ordered = Book {
        ordered: true,
        ..Book::scalar(vec![2, 2, 3, 3, 3, 4, 5, 5])
    };
    round_trip(&ordered, &[0, 1, 2, 3, 4, 5, 6, 7, 3, 1]);
}

#[test]
fn codewords_longer_than_the_lookup_table_decode() {
    // 1, 2, ..., 31, 32, 32: a complete tree as deep as Vorbis allows.
    let mut lengths: Vec<u8> = (1..=32).collect();
    lengths.push(32);
    let book = Book::scalar(lengths);
    round_trip(&book, &(0..33).collect::<Vec<_>>());
    round_trip(&book, &[32, 31, 0, 30, 9, 10, 11, 12, 20]);
    // Many long codewords of the same length.
    let mut lengths = vec![1u8];
    lengths.extend(std::iter::repeat_n(13, 1 << 12));
    let book = Book::scalar(lengths);
    round_trip(&book, &[0, 1, 4096, 2048, 0, 77, 3000]);
}

#[test]
fn a_codeword_cut_off_by_the_end_of_the_packet_is_not_decoded() {
    let book = Book::scalar(vec![1, 2, 3, 3]);
    let cb = parse(&book).unwrap();
    // 0b111 is entry 3; give it only two bits at the end of a byte.
    let mut w = BitWriter::new();
    w.put(0, 6);
    w.put(0b11, 2);
    let bytes = w.into_bytes();
    let mut r = BitReader::new(&bytes);
    r.read(6).unwrap();
    assert_eq!(cb.decode_entry(&mut r), None);
    assert_eq!(r.bits_left(), 0);
}

#[test]
fn over_and_under_specified_trees_are_refused() {
    assert!(malformed(parse(&Book::scalar(vec![1, 1, 1]))).contains("overspecified"));
    assert!(malformed(parse(&Book::scalar(vec![2, 2, 2, 3, 3, 3]))).contains("overspecified"));
    assert!(malformed(parse(&Book::scalar(vec![1, 2]))).contains("underspecified"));
    assert!(malformed(parse(&Book::scalar(vec![0, 3, 0]))).contains("underspecified"));
}

#[test]
fn a_single_entry_of_length_one_decodes_from_either_bit_as_libvorbis_does() {
    let book = Book::scalar(vec![0, 0, 1, 0]);
    let cb = parse(&book).unwrap();
    let bytes = [0b10];
    let mut r = BitReader::new(&bytes);
    assert_eq!(cb.decode_entry(&mut r), Some(2));
    assert_eq!(cb.decode_entry(&mut r), Some(2));
    assert_eq!(r.bits_left(), 6);
    let mut r = BitReader::new(&[]);
    assert_eq!(cb.decode_entry(&mut r), None);
    // Any other single length leaves the tree underspecified.
    assert!(malformed(parse(&Book::scalar(vec![0, 2]))).contains("underspecified"));
}

#[test]
fn a_book_with_no_used_entry_decodes_nothing_and_reads_nothing() {
    let cb = parse(&Book::scalar(vec![0, 0, 0])).unwrap();
    let bytes = [0xFF];
    let mut r = BitReader::new(&bytes);
    assert_eq!(cb.decode_entry(&mut r), None);
    assert_eq!(r.bits_left(), 8);
}

#[test]
fn float32_unpack_is_exact() {
    // Values from lewton's tests, which came from stb_vorbis.
    for (bits, want) in [
        (1_611_661_312u32, 1.0f32),
        (1_616_117_760, 5.0),
        (1_627_381_760, 255.0),
        (3_759_144_960, -1.0),
        (3_780_634_624, -1530.0),
        (1_628_434_432, 255.0),
        (3_781_948_416, -1530.0),
        (0, 0.0),
    ] {
        assert_eq!(float32_unpack(bits), want, "{bits}");
    }
    for v in [0.5, -0.25, 3.0, 1_234_567.0, -0.000_976_562_5] {
        assert_eq!(float32_unpack(float32_pack(v)), v as f32, "{v}");
    }
    // The extremes stay well defined: the largest exponent is infinite in f32, the smallest 0.
    assert_eq!(float32_unpack(0x7FE0_0001), f32::INFINITY);
    assert_eq!(float32_unpack(0x0000_0001), 0.0);
}

#[test]
fn lookup1_values_is_the_largest_whole_root() {
    for (entries, dims, want) in [
        (9, 2, 3),
        (8, 2, 2),
        (0, 3, 0),
        (1, 5, 1),
        (16, 4, 2),
        (15, 4, 1),
        (1000, 40, 1),
        (81, 4, 3),
        (80, 4, 2),
        ((1 << 24) - 1, 1, (1 << 24) - 1),
        (1 << 20, 2, 1 << 10),
        ((1 << 20) - 1, 2, (1 << 10) - 1),
    ] {
        assert_eq!(lookup1_values(entries, dims), want, "{entries} {dims}");
    }
}

fn values(cb: &Codebook, entry: u32, book: &Book) -> Vec<f32> {
    let mut w = BitWriter::new();
    book.encode(&mut w, entry);
    let bytes = w.into_bytes();
    cb.decode_vector(&mut BitReader::new(&bytes)).unwrap().to_vec()
}

#[test]
fn lookup_type_1_is_a_lattice_and_sequence_p_accumulates() {
    let mut book = Book::scalar(vec![4, 4, 4, 4, 4, 4, 4, 4, 1]);
    book.dims = 2;
    book.lookup = Lookup::Values {
        kind: 1,
        minimum: float32_pack(-1.0),
        delta: float32_pack(0.5),
        value_bits: 2,
        sequence: false,
        multiplicands: vec![0, 1, 2],
    };
    let cb = parse(&book).unwrap();
    for e in 0..9u32 {
        let want = [
            (e % 3) as f32 * 0.5 - 1.0,
            (e / 3) as f32 * 0.5 - 1.0,
        ];
        assert_eq!(values(&cb, e, &book), want, "entry {e}");
    }
    if let Lookup::Values { sequence, .. } = &mut book.lookup {
        *sequence = true;
    }
    let cb = parse(&book).unwrap();
    for e in 0..9u32 {
        let first = (e % 3) as f32 * 0.5 - 1.0;
        let want = [first, first + (e / 3) as f32 * 0.5 - 1.0];
        assert_eq!(values(&cb, e, &book), want, "entry {e}");
    }
}

#[test]
fn lookup_type_2_lists_every_value() {
    let book = Book {
        dims: 3,
        ..Book::flat(1)
    }
    .with_values(-2, &[0, 1, 2, 7, 6, 5]);
    let cb = parse(&book).unwrap();
    assert_eq!(cb.dims(), 3);
    assert_eq!(values(&cb, 0, &book), [-2.0, -1.0, 0.0]);
    assert_eq!(values(&cb, 1, &book), [5.0, 4.0, 3.0]);
}

#[test]
fn a_sparse_book_keeps_the_values_of_its_used_entries() {
    let book = Book {
        dims: 1,
        ..Book::scalar(vec![1, 0, 0, 1])
    }
    .with_values(0, &[10, 11, 12, 13]);
    let cb = parse(&book).unwrap();
    assert_eq!(values(&cb, 0, &book), [10.0]);
    assert_eq!(values(&cb, 3, &book), [13.0]);
}

#[test]
fn bad_codebooks_are_refused_with_a_reason() {
    let mut w = BitWriter::new();
    w.put(0x56_4343, 24);
    let bytes = w.into_bytes();
    assert!(
        malformed(Codebook::parse(
            &mut BitReader::new(&bytes),
            &mut Budget::new(1000)
        ))
        .contains("sync")
    );
    let zero_dims = Book {
        dims: 0,
        ..Book::flat(2)
    };
    assert!(malformed(parse(&zero_dims)).contains("dimension"));
    let mut bad_lookup = Book::flat(1).with_values(0, &[0, 1]);
    if let Lookup::Values { kind, .. } = &mut bad_lookup.lookup {
        *kind = 3;
    }
    assert!(malformed(parse(&bad_lookup)).contains("lookup type 3"));
    // libvorbis's bound: ilog(dimensions) + ilog(entries) at most 24.
    let wide = Book {
        dims: 1 << 9,
        ..Book::flat(15)
    };
    assert!(malformed(parse(&wide)).contains("too large"));
    // Multiplicands promised but not there.
    let mut short = Book::flat(2).with_values(0, &[1, 2, 3, 4]);
    if let Lookup::Values { multiplicands, .. } = &mut short.lookup {
        multiplicands.truncate(1);
    }
    let mut w = BitWriter::new();
    short.write(&mut w);
    let bytes = w.into_bytes();
    assert!(
        malformed(Codebook::parse(
            &mut BitReader::new(&bytes),
            &mut Budget::new(usize::MAX)
        ))
        .contains("end")
    );
}

#[test]
fn ordered_lengths_cannot_overrun_or_promise_impossible_runs() {
    // Five entries of length 2 cannot exist (at most four codewords).
    let mut w = BitWriter::new();
    w.put(0x56_4342, 24);
    w.put(1, 16);
    w.put(5, 24);
    w.put_bit(true);
    w.put(1, 5);
    w.put(5, 3);
    let bytes = w.into_bytes();
    let err = malformed(Codebook::parse(&mut BitReader::new(&bytes), &mut Budget::new(1000)));
    assert!(err.contains("length"), "{err}");
    // A run longer than the entries left.
    let mut w = BitWriter::new();
    w.put(0x56_4342, 24);
    w.put(1, 16);
    w.put(5, 24);
    w.put_bit(true);
    w.put(2, 5);
    w.put(6, 3);
    let bytes = w.into_bytes();
    let err = malformed(Codebook::parse(&mut BitReader::new(&bytes), &mut Budget::new(1000)));
    assert!(err.contains("entries"), "{err}");
    // Lengths running past 32.
    let mut w = BitWriter::new();
    w.put(0x56_4342, 24);
    w.put(1, 16);
    w.put(1, 24);
    w.put_bit(true);
    w.put(31, 5);
    w.put(0, 1);
    w.put(0, 1);
    let bytes = w.into_bytes();
    let err = malformed(Codebook::parse(&mut BitReader::new(&bytes), &mut Budget::new(1000)));
    assert!(err.contains("length"), "{err}");
}

#[test]
fn the_budget_is_checked_before_the_tables_are_built() {
    // 4096 entries of 64 dimensions: a megabyte of values from a few hundred bytes.
    let book = Book {
        dims: 64,
        ..Book::flat(12)
    };
    let mut book = book;
    book.lookup = Lookup::Values {
        kind: 1,
        minimum: 0,
        delta: float32_pack(1.0),
        value_bits: 1,
        sequence: false,
        multiplicands: vec![0],
    };
    let cb = parse_with(&book, usize::MAX).unwrap();
    assert!(cb.heap_bytes() >= 4096 * 64 * 4);
    match parse_with(&book, 200_000) {
        Err(Error::OverBudget { requested, budget }) => {
            assert_eq!(budget, 200_000);
            assert!(requested > 200_000);
        }
        other => panic!("{:?}", other.err()),
    }
    // Lengths for 2^24 entries are charged before they are read.
    let mut w = BitWriter::new();
    w.put(0x56_4342, 24);
    w.put(1, 16);
    w.put((1 << 24) - 1, 24);
    w.put_bit(true);
    w.put(0, 5);
    let bytes = w.into_bytes();
    assert!(matches!(
        Codebook::parse(&mut BitReader::new(&bytes), &mut Budget::new(1 << 20)),
        Err(Error::OverBudget { .. })
    ));
}

#[test]
fn what_a_codebook_holds_never_exceeds_what_the_budget_was_charged() {
    let mut book = Book {
        dims: 4,
        ..Book::scalar(vec![1, 0, 3, 3, 0, 3, 4, 4])
    };
    book.lookup = Lookup::Values {
        kind: 1,
        minimum: float32_pack(-3.0),
        delta: float32_pack(1.0),
        value_bits: 3,
        sequence: false,
        multiplicands: vec![0, 5],
    };
    let mut w = BitWriter::new();
    book.write(&mut w);
    let bytes = w.into_bytes();
    let mut budget = Budget::new(usize::MAX);
    let cb = Codebook::parse(&mut BitReader::new(&bytes), &mut budget).unwrap();
    assert!(cb.heap_bytes() <= budget.used(), "{} > {}", cb.heap_bytes(), budget.used());
}
