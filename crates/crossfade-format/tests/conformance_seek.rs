//! Tier 3: seeking over the IETF FLAC decoder test files (`corpus.toml`). For every file that
//! must decode, a handful of seeks (the start, the middle, the last frame, the end, backwards,
//! a few more) each give exactly the samples a decode from the start gives from that frame on:
//! `Demuxer::seek`, `Decoder::reset`, then the frames before the target dropped. `#[ignore]`d
//! like the rest of the tier; `just conformance` runs it.

use crossfade_codec::decoder_for_threads;
use crossfade_core::{AudioFrame, Demuxer, Error, Limits, Packet, Samples};
use crossfade_corpus::{Cache, Expect, Manifest, require_from, workspace_root};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A running fingerprint of a run of samples: their count and a 64-bit polynomial hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Print {
    samples: u64,
    hash: u64,
}

impl Print {
    fn add(&mut self, s: i64) {
        self.samples += 1;
        self.hash = (self.hash ^ s as u64)
            .wrapping_mul(0x0100_0000_01B3)
            .rotate_left(29);
    }
}

fn each(samples: &Samples, from: usize, mut f: impl FnMut(i64)) {
    match samples {
        Samples::U8(v) => v[from..].iter().for_each(|&s| f(i64::from(s))),
        Samples::S16(v) => v[from..].iter().for_each(|&s| f(i64::from(s))),
        Samples::S24(v) | Samples::S32(v) => v[from..].iter().for_each(|&s| f(i64::from(s))),
        other => panic!("a FLAC decoder never yields {other:?}"),
    }
}

fn open(path: &Path) -> Result<Box<dyn Demuxer>, Error> {
    let file = std::fs::File::open(path)?;
    Ok(crossfade_format::open_demuxer(Box::new(file), &Limits::default())?.1)
}

/// Decodes from where the reader is to the end, handing each frame to `on`.
fn decode_rest(
    demux: &mut dyn Demuxer,
    decoder: &mut dyn crossfade_core::Decoder,
    on: &mut dyn FnMut(&AudioFrame),
) -> Result<(), Error> {
    let stream = demux.streams()[0].clone();
    let (mut packet, mut frame) = (Packet::default(), AudioFrame::new(&stream.audio));
    while demux.read_packet(&mut packet)? {
        decoder.send_packet(&packet)?;
        while decoder.receive_frame(&mut frame)? {
            on(&frame);
        }
    }
    decoder.drain();
    while decoder.receive_frame(&mut frame)? {
        on(&frame);
    }
    Ok(())
}

/// The targets for a stream of `total` frames, in the order they are sought.
fn targets(total: u64) -> Vec<u64> {
    let mut t = vec![
        total / 2,
        total.saturating_sub(1),
        total,
        total + 10,
        total / 7,
        0,
        total * 5 / 6 + 3,
    ];
    t.retain(|&x| x <= total + 10);
    t
}

/// `Ok` or what went wrong for one file.
fn check(path: &Path) -> Result<usize, String> {
    let err = |e: Error| e.to_string();
    // The reference: the whole decode once, a fingerprint of the rest from each target.
    let mut demux = open(path).map_err(err)?;
    let stream = demux.streams()[0].clone();
    let channels = usize::from(stream.audio.layout().channels());
    let mut decoder = decoder_for_threads(&stream, 1).map_err(err)?;
    let total = match stream.frames {
        Some(n) => n,
        None => {
            let mut n = 0u64;
            decode_rest(demux.as_mut(), decoder.as_mut(), &mut |f| {
                n += f.frames() as u64;
            })
            .map_err(err)?;
            demux = open(path).map_err(err)?;
            decoder.reset();
            n
        }
    };
    let ts = targets(total);
    let mut want = vec![Print::default(); ts.len()];
    let mut at = 0u64;
    decode_rest(demux.as_mut(), decoder.as_mut(), &mut |f| {
        let end = at + f.frames() as u64;
        for (t, w) in ts.iter().zip(&mut want) {
            if end > *t {
                let skip = t.saturating_sub(at) as usize * channels;
                each(&f.samples, skip, |s| w.add(s));
            }
        }
        at = end;
    })
    .map_err(err)?;
    if at != total {
        return Err(format!("decoded {at} frames, the stream says {total}"));
    }
    // The seeks: one reader, one decoder, every other seek on the automatic thread count.
    let mut demux = open(path).map_err(err)?;
    let mut decoders = [
        decoder_for_threads(&stream, 1).map_err(err)?,
        decoder_for_threads(&stream, 0).map_err(err)?,
    ];
    for (i, (&t, w)) in ts.iter().zip(&want).enumerate() {
        let landed = demux.seek(t as i64).map_err(err)?;
        if landed as u64 > t.min(total) {
            return Err(format!("seek {t} landed at {landed}, after the target"));
        }
        let decoder = &mut decoders[i % 2];
        decoder.reset();
        let mut got = Print::default();
        let mut at = landed as u64;
        decode_rest(demux.as_mut(), decoder.as_mut(), &mut |f| {
            let end = at + f.frames() as u64;
            if end > t {
                let skip = t.saturating_sub(at) as usize * channels;
                each(&f.samples, skip, |s| got.add(s));
            }
            at = end;
        })
        .map_err(|e| format!("seek {t}: {e}"))?;
        if got != *w {
            return Err(format!(
                "seek {t}: {} samples, want {}, or they differ",
                got.samples, w.samples
            ));
        }
    }
    Ok(ts.len())
}

#[test]
#[ignore = "needs the corpus: `just corpus`, then `just conformance`"]
fn seeking_any_file_that_decodes_gives_the_samples_a_decode_from_the_start_gives() {
    let root = workspace_root();
    let manifest = Manifest::load(&root.join("corpus.toml")).expect("corpus.toml");
    let cache = Cache::new(Cache::dir_from_env(&root));
    let entries: Vec<_> = manifest
        .entries()
        .iter()
        .filter(|e| e.expect == Expect::Decode)
        .collect();
    let paths: Vec<_> = entries
        .iter()
        .map(|e| require_from(&manifest, &cache, &e.id))
        .collect();
    let failures = Mutex::new(Vec::new());
    let seeks = AtomicUsize::new(0);
    let next = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism().map_or(2, usize::from);
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = paths.get(i) else {
                        break;
                    };
                    match check(path) {
                        Ok(n) => {
                            seeks.fetch_add(n, Ordering::Relaxed);
                        }
                        Err(why) => failures
                            .lock()
                            .unwrap()
                            .push(format!("{}: {why}", entries[i].id)),
                    }
                }
            });
        }
    });
    let failures = failures.into_inner().unwrap();
    assert!(
        failures.is_empty(),
        "{} of {} files failed:\n{}",
        failures.len(),
        paths.len(),
        failures.join("\n")
    );
    eprintln!(
        "{} seeks over {} files",
        seeks.load(Ordering::Relaxed),
        paths.len()
    );
}
