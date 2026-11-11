//! In-process FLAC read timing, with the file already in memory so disk and process start do
//! not count: `cargo run --release -p crossfade-format --example profile_flac_read -- [--passes
//! N] [--loop MODE] FILE...`
//!
//! Per file it prints the best of N passes of: `split` (the demuxer alone, every packet read),
//! `decode` (the standalone decoder with its CRC-16 and MD5 checks over packets split
//! beforehand) and `read` (demux, decode and verify together, the samples discarded).
//! `--loop MODE` repeats one mode on the first file forever, for a sampling profiler.
//! `--threads N` decodes through `push_frame` on N threads (0: automatic); without it, one
//! frame at a time through `decode_frame`.

use crossfade_core::{Limits, Packet};
use crossfade_flac::Decoder;
use crossfade_flac::streaminfo::StreamInfoBlock;
use std::io::Cursor;
use std::time::Instant;

fn demuxer(bytes: Vec<u8>) -> Box<dyn crossfade_core::Demuxer> {
    crossfade_format::open_demuxer(Box::new(Cursor::new(bytes)), &Limits::default())
        .expect("a FLAC file")
        .1
}

fn info(d: &dyn crossfade_core::Demuxer) -> StreamInfoBlock {
    StreamInfoBlock::parse(&d.streams()[0].extradata).expect("STREAMINFO")
}

fn split(bytes: Vec<u8>) -> usize {
    let mut d = demuxer(bytes);
    let mut p = Packet::default();
    let mut n = 0;
    while d.read_packet(&mut p).expect("demux") {
        n += p.data.len();
    }
    n
}

/// One frame into `dec`: through `decode_frame`, or `push_frame` when `threads` is set.
fn feed(dec: &mut Decoder, frame: &[u8], threads: Option<usize>, n: &mut usize) {
    if threads.is_some() {
        dec.push_frame(frame, &mut |b| *n += b.len())
            .expect("decode");
    } else {
        *n += dec.decode_frame(frame).expect("decode").len();
    }
}

fn open(info: &StreamInfoBlock, threads: Option<usize>) -> Decoder {
    Decoder::with_threads(info, threads.unwrap_or(1)).expect("decoder")
}

fn decode(info: &StreamInfoBlock, packets: &[Vec<u8>], threads: Option<usize>) -> usize {
    let mut dec = open(info, threads);
    let mut n = 0;
    for p in packets {
        feed(&mut dec, p, threads, &mut n);
    }
    dec.flush(&mut |b| n += b.len()).expect("decode");
    dec.finish().expect("verify");
    n
}

fn read(bytes: Vec<u8>, threads: Option<usize>) -> usize {
    let mut d = demuxer(bytes);
    let mut dec = open(&info(d.as_ref()), threads);
    let mut p = Packet::default();
    let mut n = 0;
    while d.read_packet(&mut p).expect("demux") {
        feed(&mut dec, &p.data, threads, &mut n);
    }
    dec.flush(&mut |b| n += b.len()).expect("decode");
    dec.finish().expect("verify");
    n
}

/// Best wall time in ms of `passes` runs; `prepare` runs outside the clock.
fn best<P>(passes: usize, mut prepare: impl FnMut() -> P, mut run: impl FnMut(P) -> usize) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..passes {
        let input = prepare();
        let t = Instant::now();
        std::hint::black_box(run(input));
        best = best.min(t.elapsed().as_secs_f64() * 1000.0);
    }
    best
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut passes, mut looping, mut files, mut threads) = (20, None, Vec::new(), None);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--passes" => passes = args.next().and_then(|v| v.parse().ok()).expect("a count"),
            "--loop" => looping = args.next(),
            "--threads" => threads = args.next().and_then(|v| v.parse().ok()),
            _ => files.push(a),
        }
    }
    let load = |f: &String| std::fs::read(f).expect("readable file");
    let packets = |bytes: Vec<u8>| {
        let mut d = demuxer(bytes);
        let info = info(d.as_ref());
        let mut out = Vec::new();
        let mut p = Packet::default();
        while d.read_packet(&mut p).expect("demux") {
            out.push(p.data.clone());
        }
        (info, out)
    };
    if let Some(mode) = looping {
        let bytes = load(&files[0]);
        let (info, pk) = packets(bytes.clone());
        loop {
            std::hint::black_box(match mode.as_str() {
                "split" => split(bytes.clone()),
                "decode" => decode(&info, &pk, threads),
                _ => read(bytes.clone(), threads),
            });
        }
    }
    println!(
        "{:<28}{:>10}{:>10}{:>10}",
        "file", "split ms", "decode ms", "read ms"
    );
    for f in &files {
        let bytes = load(f);
        let (info, pk) = packets(bytes.clone());
        // Interleaved so a burst of load elsewhere hits all three alike.
        let (mut s, mut d, mut r) = (f64::MAX, f64::MAX, f64::MAX);
        for _ in 0..passes {
            s = s.min(best(1, || bytes.clone(), split));
            d = d.min(best(1, || (), |()| decode(&info, &pk, threads)));
            r = r.min(best(1, || bytes.clone(), |b| read(b, threads)));
        }
        let name = f.rsplit('/').next().unwrap_or(f);
        println!("{name:<28}{s:>10.2}{d:>10.2}{r:>10.2}");
    }
}
