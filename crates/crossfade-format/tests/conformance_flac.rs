//! Tier 3: the IETF FLAC decoder test files (`corpus.toml`) through the native reader and
//! decoder. Every test that needs the corpus is `#[ignore]`d so a plain `cargo test` stays
//! offline; `just conformance` runs them, and they fail (never skip) when a file is missing.
//!
//! Each file is held to its `expect`, and the whole run is compared with the frozen manifest
//! `tests/conformance/flac.txt` (`CROSSFADE_BLESS=1` rewrites it) so a later decoder
//! optimisation cannot change a single output bit unnoticed.

use crossfade_codec::decoder_for_threads;
use crossfade_core::{AudioFrame, Limits, Packet, Samples};
use crossfade_corpus::{Cache, Entry, Expect, Manifest, require_from, try_require, workspace_root};
use crossfade_flac::md5::Md5;
use sha2::{Digest, Sha256};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};

const FROZEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/conformance/flac.txt");

struct Run {
    /// `Ok` or the display text of the first error of any stage.
    outcome: Result<(), String>,
    packets: u64,
    frames: u64,
    /// SHA-256 of the output as raw little-endian PCM in the container sample format.
    sha256: String,
    /// MD5 over the samples with the left-justification undone (RFC 9639 section 8.2).
    md5: [u8; 16],
    /// STREAMINFO's total sample count; `None` when unknown.
    total: Option<u64>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Appends the samples as the container holds them (24-bit packed in three bytes).
fn container_bytes(samples: &Samples, out: &mut Vec<u8>) {
    match samples {
        Samples::U8(v) => out.extend_from_slice(v),
        Samples::S16(v) => v.iter().for_each(|s| out.extend(s.to_le_bytes())),
        Samples::S24(v) => v.iter().for_each(|s| out.extend(&s.to_le_bytes()[..3])),
        Samples::S32(v) => v.iter().for_each(|s| out.extend(s.to_le_bytes())),
        other => panic!("a FLAC decoder never yields {other:?}"),
    }
}

/// Appends the samples as the stream stores them: `ceil(bits/8)` little-endian signed bytes.
fn stream_bytes(samples: &Samples, bits: u32, out: &mut Vec<u8>) {
    let width = bits.div_ceil(8) as usize;
    let mut put = |v: i64| out.extend_from_slice(&v.to_le_bytes()[..width]);
    match samples {
        Samples::U8(v) => v
            .iter()
            .for_each(|&s| put((i64::from(s) - 128) >> (8 - bits))),
        Samples::S16(v) => v.iter().for_each(|&s| put(i64::from(s) >> (16 - bits))),
        Samples::S24(v) => v.iter().for_each(|&s| put(i64::from(s) >> (24 - bits))),
        Samples::S32(v) => v.iter().for_each(|&s| put(i64::from(s) >> (32 - bits))),
        other => panic!("a FLAC decoder never yields {other:?}"),
    }
}

/// Decodes the file through the engine's FLAC decoder on `threads` threads (0: automatic).
fn run(path: &Path, threads: usize) -> Run {
    let mut r = Run {
        outcome: Ok(()),
        packets: 0,
        frames: 0,
        sha256: String::new(),
        md5: [0; 16],
        total: None,
    };
    let (mut sha, mut md5) = (Sha256::new(), Md5::new());
    let result = (|| -> Result<(), crossfade_core::Error> {
        let file = std::fs::File::open(path)?;
        let (_, mut demux) = crossfade_format::open_demuxer(Box::new(file), &Limits::default())?;
        let stream = demux.streams()[0].clone();
        r.total = stream.frames;
        let bits = u32::from(
            stream
                .bits_per_raw_sample
                .unwrap_or(stream.audio.format().bits() as u8),
        );
        let mut decoder = decoder_for_threads(&stream, threads)?;
        let (mut packet, mut frame) = (Packet::default(), AudioFrame::new(&stream.audio));
        let (mut a, mut b) = (Vec::new(), Vec::new());
        let mut take = |frame: &AudioFrame, r: &mut Run| {
            r.frames += frame.frames() as u64;
            a.clear();
            b.clear();
            container_bytes(&frame.samples, &mut a);
            stream_bytes(&frame.samples, bits, &mut b);
            sha.update(&a);
            md5.update(&b);
        };
        loop {
            match demux.read_packet(&mut packet) {
                Ok(true) => {}
                Ok(false) => break,
                Err(e) => {
                    // As the pipeline does: the frames read before the bad packet count, even
                    // those a threaded decoder still holds, and an error among them comes first.
                    decoder.flush()?;
                    while decoder.receive_frame(&mut frame)? {
                        take(&frame, &mut r);
                    }
                    return Err(e);
                }
            }
            r.packets += 1;
            decoder.send_packet(&packet)?;
            while decoder.receive_frame(&mut frame)? {
                take(&frame, &mut r);
            }
        }
        decoder.drain();
        while decoder.receive_frame(&mut frame)? {
            take(&frame, &mut r);
        }
        Ok(())
    })();
    r.outcome = result.map_err(|e| e.to_string());
    r.sha256 = hex(&sha.finalize());
    r.md5 = md5.finalize();
    r
}

/// Entry, then the run or the panic message.
type Result3 = (Entry, Result<Run, String>);

fn panic_text(p: Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<String>()
        .cloned()
        .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_else(|| "non-string panic".into())
}

/// Every entry once per process, decoded on one thread, in manifest order, spread over the
/// cores.
static RESULTS: LazyLock<Vec<Result3>> = LazyLock::new(|| run_all(1));
/// The same with the decoder's automatic thread count: the results must not differ.
static RESULTS_AUTO: LazyLock<Vec<Result3>> = LazyLock::new(|| run_all(0));

fn run_all(threads: usize) -> Vec<Result3> {
    let root = workspace_root();
    let manifest = Manifest::load(&root.join("corpus.toml")).expect("corpus.toml");
    let cache = Cache::new(Cache::dir_from_env(&root));
    // The corpus holds other codecs' vectors too; this tier is the FLAC suite.
    let entries: Vec<&Entry> = manifest
        .entries()
        .iter()
        .filter(|e| e.id.starts_with("flac/"))
        .collect();
    // Resolve (and verify) up front: a missing corpus fails the tier outright.
    let paths: Vec<_> = entries
        .iter()
        .map(|e| require_from(&manifest, &cache, &e.id))
        .collect();
    let slots: Vec<Mutex<Option<Result<Run, String>>>> =
        entries.iter().map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism().map_or(2, usize::from);
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= entries.len() {
                        break;
                    }
                    let got = catch_unwind(AssertUnwindSafe(|| run(&paths[i], threads)))
                        .map_err(panic_text);
                    *slots[i].lock().unwrap() = Some(got);
                }
            });
        }
    });
    entries
        .into_iter()
        .cloned()
        .zip(slots)
        .map(|(e, s)| (e, s.into_inner().unwrap().expect("every entry ran")))
        .collect()
}

fn check(entry: &Entry, got: &Result<Run, String>) -> Vec<String> {
    let run = match got {
        Err(p) => return vec![format!("panicked: {p}")],
        Ok(r) => r,
    };
    let mut bad = Vec::new();
    match (&run.outcome, entry.expect) {
        (_, Expect::NoPanic) => return bad,
        (Err(e), Expect::Decode) => return vec![format!("must decode, got: {e}")],
        (Err(_), _) => return bad,
        (Ok(()), _) => {}
    }
    if let Some(want) = &entry.md5 {
        let have = hex(&run.md5);
        if have != *want {
            bad.push(format!("recomputed MD5 {have}, manifest says {want}"));
        }
    }
    if let Some(total) = run.total
        && total != run.frames
    {
        bad.push(format!(
            "decoded {} frames, STREAMINFO says {total}",
            run.frames
        ));
    }
    bad
}

#[test]
#[ignore = "needs the corpus: `just corpus`, then `just conformance`"]
fn every_entry_meets_its_expectation() {
    let failures: Vec<String> = RESULTS
        .iter()
        .flat_map(|(e, got)| {
            check(e, got)
                .into_iter()
                .map(move |why| format!("{} ({}): {why}", e.id, e.expect.as_str()))
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {} entries failed:\n{}",
        failures.len(),
        RESULTS.len(),
        failures.join("\n")
    );
}

fn frozen_line(entry: &Entry, got: &Result<Run, String>) -> String {
    match got {
        Err(p) => format!("{}\tPANIC {p}\t-\t-\t-", entry.id),
        Ok(r) => format!(
            "{}\t{}\t{}\t{}\t{}",
            entry.id,
            r.outcome
                .as_ref()
                .map_or_else(Clone::clone, |()| "ok".into()),
            r.packets,
            r.frames,
            r.sha256
        ),
    }
}

const FROZEN_HEADER: &str = "\
# Frozen results of the FLAC conformance run (tests/conformance_flac.rs); do not edit by hand.
# Rewrite with `CROSSFADE_BLESS=1 just conformance` after a deliberate decoder change.
# Columns (tab-separated): entry id, outcome (ok or the error's text), packets read,
# frames decoded, SHA-256 of the decoded audio as raw little-endian PCM in the container
# sample format (8-bit unsigned, 16-bit, 24-bit packed in three bytes, 32-bit).
";

#[test]
#[ignore = "needs the corpus: `just corpus`, then `just conformance`"]
fn results_match_the_frozen_manifest() {
    compare_with_frozen(&RESULTS, true);
}

/// Threads change nothing: not the samples, not the error, not how far the reader got.
#[test]
#[ignore = "needs the corpus: `just corpus`, then `just conformance`"]
fn results_on_automatic_threads_match_the_frozen_manifest() {
    compare_with_frozen(&RESULTS_AUTO, false);
}

/// `bless`: this run may rewrite the manifest when `CROSSFADE_BLESS` is set (the one-thread
/// run only, so the two runs are always held to the same file).
fn compare_with_frozen(results: &[Result3], bless: bool) {
    let mut text = String::from(FROZEN_HEADER);
    for (e, got) in results {
        text.push_str(&frozen_line(e, got));
        text.push('\n');
    }
    if bless && std::env::var_os("CROSSFADE_BLESS").is_some() {
        std::fs::write(FROZEN, &text).expect("write the frozen manifest");
        return;
    }
    let frozen = std::fs::read_to_string(FROZEN)
        .unwrap_or_else(|e| panic!("{FROZEN}: {e}; create it with CROSSFADE_BLESS=1"));
    let lines = |t: &str| -> Vec<String> {
        t.lines()
            .filter(|l| !l.starts_with('#'))
            .map(str::to_owned)
            .collect()
    };
    let (want, have) = (lines(&frozen), lines(&text));
    let diffs: Vec<String> = want
        .iter()
        .zip(&have)
        .filter(|(w, h)| w != h)
        .map(|(w, h)| format!("frozen: {w}\n   now: {h}"))
        .collect();
    assert!(
        want.len() == have.len() && diffs.is_empty(),
        "frozen manifest has {} lines, this run {}; {} differ:\n{}",
        want.len(),
        have.len(),
        diffs.len(),
        diffs.join("\n")
    );
}

/// Runs in every `cargo test`: an absent corpus must fail the tier with the instruction,
/// not skip. Uses an empty cache directory rather than the environment.
#[test]
fn a_missing_corpus_fails_and_says_to_run_just_corpus() {
    let root = workspace_root();
    let manifest = Manifest::load(&root.join("corpus.toml")).expect("corpus.toml");
    let dir = std::env::temp_dir().join(format!("crossfade-empty-corpus-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cache = Cache::new(dir.clone());
    let id = &manifest.entries()[0].id;
    let err = try_require(&manifest, &cache, id).unwrap_err().to_string();
    assert!(err.contains("just corpus"), "{err}");
    let panic = catch_unwind(|| require_from(&manifest, &cache, id)).unwrap_err();
    assert!(panic_text(panic).contains("just corpus"));
    std::fs::remove_dir_all(dir).ok();
}
