//! In-process decode speed against lewton on the same packets, one thread, best of N runs.
//!
//! `cargo run --release -p crossfade-vorbis --example vs_lewton -- <file.ogg>... [--runs 5] [--ours]`
//! (with `--features profile`, the share of each decode stage too)
//!
//! Packets are read from the Ogg file once, up front; each run decodes every logical stream
//! from its headers on, keeping the samples (as a caller would) in a reused buffer.

use crossfade_vorbis::{Decoder, Ident, Setup};
use lewton::audio::{PreviousWindowRight, read_audio_packet_generic};
use lewton::header::{read_header_ident, read_header_setup};
use lewton::samples::InterleavedSamples;
use ogg::reading::PacketReader;
use std::time::{Duration, Instant};

fn links(bytes: &[u8]) -> Vec<Vec<Vec<u8>>> {
    let mut rd = PacketReader::new(std::io::Cursor::new(bytes.to_vec()));
    let mut out: Vec<(u32, Vec<Vec<u8>>)> = vec![];
    while let Ok(Some(p)) = rd.read_packet() {
        let serial = p.stream_serial();
        if p.first_in_stream() || !out.iter().any(|(s, _)| *s == serial) {
            out.push((serial, vec![]));
        }
        if let Some((_, l)) = out.iter_mut().rev().find(|(s, _)| *s == serial) {
            l.push(p.data);
        }
    }
    out.into_iter().map(|(_, l)| l).collect()
}

/// Where this crate's decode spent its time, summed over every stream (feature `profile`).
#[cfg(feature = "profile")]
static STAGE_TIMES: std::sync::Mutex<[Duration; 6]> = std::sync::Mutex::new([Duration::ZERO; 6]);

fn ours(links: &[Vec<Vec<u8>>], out: &mut Vec<f32>) -> usize {
    let mut samples = 0;
    for l in links {
        let ident = Ident::parse(&l[0]).unwrap();
        let setup = Setup::parse(&l[2], &ident, 256 << 20).unwrap();
        let mut d = Decoder::new(&ident, setup).unwrap();
        for p in &l[3..] {
            out.clear();
            if d.decode(p, out).is_ok() {
                samples += out.len();
            }
        }
        #[cfg(feature = "profile")]
        {
            let mut t = STAGE_TIMES.lock().unwrap();
            for (a, b) in t.iter_mut().zip(d.stage_times()) {
                *a += b;
            }
        }
    }
    samples
}

fn theirs(links: &[Vec<Vec<u8>>]) -> usize {
    let mut samples = 0;
    for l in links {
        let id = read_header_ident(&l[0]).unwrap();
        let setup =
            read_header_setup(&l[2], id.audio_channels, (id.blocksize_0, id.blocksize_1)).unwrap();
        let mut pwr = PreviousWindowRight::new();
        for p in &l[3..] {
            if let Ok(d) =
                read_audio_packet_generic::<InterleavedSamples<f32>>(&id, &setup, p, &mut pwr)
            {
                samples += d.samples.len();
            }
        }
    }
    samples
}

fn best(runs: usize, mut f: impl FnMut() -> usize) -> (Duration, usize) {
    let mut best = Duration::MAX;
    let mut n = 0;
    for _ in 0..runs {
        let t = Instant::now();
        n = std::hint::black_box(f());
        best = best.min(t.elapsed());
    }
    (best, n)
}

fn main() {
    let mut runs = 5;
    let mut files = vec![];
    let mut only_ours = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--ours" {
            // For a profiler: this crate only.
            only_ours = true;
        } else if a == "--runs" {
            runs = args.next().and_then(|r| r.parse().ok()).expect("--runs N");
        } else {
            files.push(a);
        }
    }
    println!(
        "| file | samples | crossfade-vorbis | lewton | lewton / ours | ours Msamples/s |\n|---|---:|---:|---:|---:|---:|"
    );
    let mut out = Vec::new();
    for f in files {
        let bytes = std::fs::read(&f).unwrap();
        let ls = links(&bytes);
        let (t_ours, n_ours) = best(runs, || ours(&ls, &mut out));
        let (t_theirs, _) = if only_ours {
            (t_ours, 0)
        } else {
            best(runs, || theirs(&ls))
        };
        let name = std::path::Path::new(&f)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        #[cfg(feature = "profile")]
        {
            let mut t = STAGE_TIMES.lock().unwrap();
            let total: Duration = t.iter().sum();
            let shares: Vec<String> = crossfade_vorbis::STAGES
                .iter()
                .zip(t.iter())
                .map(|(s, d)| format!("{s} {:.0}%", d.as_secs_f64() / total.as_secs_f64() * 100.0))
                .collect();
            eprintln!("{name}: {}", shares.join(", "));
            *t = [Duration::ZERO; 6];
        }
        println!(
            "| {name} | {n_ours} | {:.1} ms | {:.1} ms | {:.2}x | {:.1} |",
            t_ours.as_secs_f64() * 1e3,
            t_theirs.as_secs_f64() * 1e3,
            t_theirs.as_secs_f64() / t_ours.as_secs_f64(),
            n_ours as f64 / t_ours.as_secs_f64() / 1e6
        );
    }
}
