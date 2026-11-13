//! Seeking in Ogg FLAC: the frame that holds the target, then the same packets a read from the
//! start gives, through the granule bisection; in O(log n) reads.

mod seek_support;

use crossfade_core::{Error, Limits, Packet, Tags};
use crossfade_format::{FormatId, create_muxer, open_demuxer};
use crossfade_testkit::io::FaultySource;
use seek_support::{
    Counting, Lcg, Shared, Table, check_all_targets, drain, flac_file, open_bytes, signal,
};
use std::io::Cursor;

fn fixtures() -> Vec<(String, Vec<u8>)> {
    let dir = format!("{}/tests/fixtures/ogg", env!("CARGO_MANIFEST_DIR"));
    let mut names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "oga"))
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|p| (p.display().to_string(), std::fs::read(&p).unwrap()))
        .collect()
}

/// The native FLAC file's frames in Ogg, through the engine's own writer.
fn to_ogg(flac: &[u8]) -> Vec<u8> {
    let mut d = open_bytes(flac);
    let stream = d.streams()[0].clone();
    let sink = Shared::default();
    let mut m = create_muxer(FormatId::Ogg, Box::new(sink.clone()), &stream, &Tags::new()).unwrap();
    for p in drain(d.as_mut()).unwrap() {
        m.write_packet(&p).unwrap();
    }
    m.finish().unwrap();
    sink.bytes()
}

#[test]
fn every_ogg_fixture_seeks_to_the_frame_that_holds_the_target() {
    for (name, bytes) in fixtures() {
        // A file whose stream is not FLAC has nothing to seek.
        if open_demuxer(Box::new(Cursor::new(bytes.clone())), &Limits::default()).is_err() {
            continue;
        }
        check_all_targets(&bytes, &name);
    }
}

#[test]
fn synthetic_streams_seek_in_ogg() {
    for (channels, bits) in [(1u8, 16u8), (2, 24), (6, 16)] {
        let samples = signal(30_011, usize::from(channels), bits, u64::from(bits) + 1);
        for blocks in [None, Some(&[1152usize, 37, 4096, 576, 2000][..])] {
            let f = flac_file(&samples, channels, bits, blocks, Table::None, true);
            check_all_targets(
                &to_ogg(&f.bytes),
                &format!("{channels}ch {bits}bit {blocks:?}"),
            );
        }
    }
}

#[test]
fn a_reader_on_a_pipe_cannot_seek_and_says_so() {
    let bytes = std::fs::read(format!(
        "{}/tests/fixtures/ogg/stereo16.oga",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let pipe = FaultySource::new(Cursor::new(bytes)).not_seekable();
    let (_, mut d) = open_demuxer(Box::new(pipe), &Limits::default()).unwrap();
    assert!(matches!(d.seek(0), Err(Error::Unsupported { .. })));
    assert!(d.read_packet(&mut Packet::default()).unwrap());
}

/// A seek reads O(log n) pages, not the file.
#[test]
fn a_seek_reads_a_logarithmic_number_of_bytes() {
    let samples = signal(1_500_000, 1, 16, 11);
    let f = flac_file(&samples, 1, 16, None, Table::None, true);
    let ogg = to_ogg(&f.bytes);
    let (source, counter) = Counting::new(ogg.clone());
    let (_, mut d) = open_demuxer(Box::new(source), &Limits::default()).unwrap();
    let log2 = 64 - u64::from((ogg.len() as u64).leading_zeros());
    // Pages close at 4 KiB of body, so a page is at most that plus a frame.
    let page = 4096 + f.max_frame as u64 + 282;
    let bound = 8 * page * log2;
    let mut rng = Lcg(5);
    let mut worst = 0;
    counter.take();
    for _ in 0..40 {
        d.seek(rng.below(1_500_000) as i64).unwrap();
        d.read_packet(&mut Packet::default()).unwrap();
        worst = worst.max(counter.take().0);
    }
    eprintln!(
        "{} byte file, pages up to {page} bytes: worst seek read {worst} bytes (bound {bound})",
        ogg.len()
    );
    assert!(worst <= bound, "{worst} > {bound}");
}
