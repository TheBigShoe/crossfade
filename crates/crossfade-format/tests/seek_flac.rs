//! Seeking in native FLAC: the frame that holds the target, then the same packets a read from
//! the start gives; with and without a SEEKTABLE (a wrong one is not trusted), fixed and
//! variable block sizes; in O(log n) reads.

mod seek_support;

use crossfade_core::{Error, Limits, Packet};
use crossfade_format::open_demuxer;
use crossfade_testkit::io::FaultySource;
use seek_support::{Counting, Lcg, Table, check_all_targets, flac_file, open_bytes, signal};
use std::io::Cursor;

fn fixtures() -> Vec<(String, Vec<u8>)> {
    let root = format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"));
    let mut out = Vec::new();
    for dir in ["flac", "ogg"] {
        let mut names: Vec<_> = std::fs::read_dir(format!("{root}/{dir}"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "flac"))
            .collect();
        names.sort();
        for p in names {
            out.push((p.display().to_string(), std::fs::read(&p).unwrap()));
        }
    }
    out
}

#[test]
fn every_flac_fixture_seeks_to_the_frame_that_holds_the_target() {
    for (name, bytes) in fixtures() {
        check_all_targets(&bytes, &name);
    }
}

#[test]
fn synthetic_streams_seek_with_and_without_a_seektable() {
    for (channels, bits) in [(1u8, 16u8), (2, 24), (6, 16)] {
        let samples = signal(30_011, usize::from(channels), bits, u64::from(bits));
        for blocks in [None, Some(&[1152usize, 37, 4096, 576, 2000][..])] {
            for table in [Table::None, Table::Every(3), Table::Every(1), Table::Wrong] {
                for total in [true, false] {
                    let f = flac_file(&samples, channels, bits, blocks, table, total);
                    let what = format!("{channels}ch {bits}bit {blocks:?} {table:?} total {total}");
                    check_all_targets(&f.bytes, &what);
                }
            }
        }
    }
}

#[test]
fn a_reader_on_a_pipe_cannot_seek_and_says_so() {
    let (_, bytes) = &fixtures()[0];
    let pipe = FaultySource::new(Cursor::new(bytes.clone())).not_seekable();
    let (_, mut d) = open_demuxer(Box::new(pipe), &Limits::default()).unwrap();
    assert!(matches!(d.seek(0), Err(Error::Unsupported { .. })));
    // And reading still works.
    assert!(d.read_packet(&mut Packet::default()).unwrap());
}

#[test]
fn a_stream_with_no_frames_is_at_its_end_at_once() {
    let f = flac_file(&signal(100, 1, 16, 1), 1, 16, None, Table::None, true);
    let first = f.bytes.len() - f.frames.iter().map(|(_, n)| n).sum::<usize>();
    let mut d = open_bytes(&f.bytes[..first]);
    assert_eq!(d.seek(50).unwrap(), 0);
    assert!(!d.read_packet(&mut Packet::default()).unwrap());
}

/// A seek reads O(log n) bytes: a few frames for each halving of the file, never a scan.
#[test]
fn a_seek_reads_a_logarithmic_number_of_bytes() {
    let samples = signal(1_500_000, 1, 16, 7);
    for (blocks, table) in [
        (None, Table::None),
        (Some(&[4096usize, 1000, 3000][..]), Table::None),
        (None, Table::Wrong),
    ] {
        let f = flac_file(&samples, 1, 16, blocks, table, true);
        let (source, counter) = Counting::new(f.bytes.clone());
        let (_, mut d) = open_demuxer(Box::new(source), &Limits::default()).unwrap();
        let log2 = 64 - u64::from((f.bytes.len() as u64).leading_zeros());
        let bound = 8 * (f.max_frame as u64 + 8192) * log2;
        let mut rng = Lcg(3);
        let mut worst = 0;
        counter.take();
        for _ in 0..40 {
            let t = rng.below(1_500_000) as i64;
            d.seek(t).unwrap();
            d.read_packet(&mut Packet::default()).unwrap();
            let (bytes, _) = counter.take();
            worst = worst.max(bytes);
        }
        eprintln!(
            "{blocks:?} {table:?}: {} byte file, max frame {}, worst seek read {worst} bytes (bound {bound})",
            f.bytes.len(),
            f.max_frame
        );
        assert!(worst <= bound, "{worst} > {bound}");
        assert!(worst < f.bytes.len() as u64 / 8);
    }
}

/// With a point at every frame, the table alone finds the frame.
#[test]
fn a_dense_seektable_is_used() {
    let samples = signal(400_000, 2, 16, 9);
    let f = flac_file(&samples, 2, 16, None, Table::Every(1), true);
    let (source, counter) = Counting::new(f.bytes.clone());
    let (_, mut d) = open_demuxer(Box::new(source), &Limits::default()).unwrap();
    let mut rng = Lcg(4);
    counter.take();
    for _ in 0..20 {
        d.seek(rng.below(400_000) as i64).unwrap();
        let (bytes, _) = counter.take();
        assert!(
            bytes <= 4 * (f.max_frame as u64 + 8192),
            "{bytes} bytes read for one seek"
        );
    }
}
