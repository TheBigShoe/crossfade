//! Tier 3: the RFC 8251 Opus test vectors (`opus/rfc8251/*` in `corpus.toml`) through
//! `crossfade_opus::Decoder` at 48 kHz stereo, as `run_vectors.sh` runs `opus_demo -d 48000 2`
//! and `opus_compare -s`. `#[ignore]`d so a plain `cargo test` stays offline; they fail, never
//! skip, when a file is missing. Not yet part of `just conformance`: the decoder is built in
//! stages, and this tier joins the gate when every vector passes.
//!
//! For every packet the decoder's final range must equal the encoder's stored in the `.bit`
//! file (the strict test of the entropy decoding: the first mismatch is reported with its
//! packet index and TOC); then the output must pass `opus_compare` against `.dec` or `m.dec`
//! (RFC 8251 section 11: either set).
//!
//! `OPUS_VECTORS=01,07` runs only those vectors; `OPUS_MODES=celt` (or `silk`, `hybrid`, a
//! comma-separated list) only the vectors whose packets all use the modes listed (see
//! `crates/crossfade-opus/tests/VECTORS.md`); `OPUS_PACKETS=1` prints every packet.

use crossfade_opus::testing::bitfile::read_pcm16;
use crossfade_opus::testing::compare::compare_pcm16;
use crossfade_opus::testing::vectors::{decode_vector, modes};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

const VECTORS: [&str; 12] = [
    "01", "02", "03", "04", "05", "06", "07", "08", "09", "10", "11", "12",
];

fn corpus(name: &str) -> Vec<u8> {
    let path = crossfade_corpus::require(&format!("opus/rfc8251/{name}"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn env_list(name: &str) -> Option<Vec<String>> {
    std::env::var(name)
        .ok()
        .map(|v| v.split(',').map(|s| s.trim().to_lowercase()).collect())
}

/// The vectors selected by `OPUS_VECTORS` and `OPUS_MODES`.
fn selected() -> Vec<(&'static str, Vec<u8>)> {
    let only = env_list("OPUS_VECTORS");
    let modes_wanted = env_list("OPUS_MODES");
    VECTORS
        .iter()
        .filter(|n| only.as_ref().is_none_or(|o| o.iter().any(|v| v == *n)))
        .map(|&n| (n, corpus(&format!("testvector{n}.bit"))))
        .filter(|(_, bit)| {
            modes_wanted.as_ref().is_none_or(|wanted| {
                modes(bit)
                    .unwrap()
                    .iter()
                    .all(|m| wanted.iter().any(|w| w == m))
            })
        })
        .collect()
}

/// `f` over every item on as many threads as the machine has (each vector takes seconds in
/// `opus_compare`), the results in the items' order.
fn each<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let next = AtomicUsize::new(0);
    let results = Mutex::new((0..items.len()).map(|_| None).collect::<Vec<_>>());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|s| {
        for _ in 0..threads.min(items.len()) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(i) else { break };
                    let r = f(item);
                    results.lock().unwrap()[i] = Some(r);
                }
            });
        }
    });
    results
        .into_inner()
        .unwrap()
        .into_iter()
        .map(Option::unwrap)
        .collect()
}

/// `Ok(summary)` when the vector passes, `Err(why)` otherwise.
fn run(n: &str, bit: &[u8]) -> Result<String, String> {
    let print = std::env::var_os("OPUS_PACKETS").is_some();
    let pcm = decode_vector(bit, 2, |line| {
        if print {
            println!("testvector{n} {line}");
        }
    })?;
    let mut figures = vec![];
    let mut passes = false;
    for suffix in [".dec", "m.dec"] {
        let reference = read_pcm16(&corpus(&format!("testvector{n}{suffix}")));
        let q = compare_pcm16(&reference, &pcm, true, 48000)?;
        passes |= q.passes();
        figures.push(format!("Q {:.1} against {suffix}", q.q));
    }
    let figures = figures.join(", ");
    if passes {
        Ok(figures)
    } else {
        Err(format!("opus_compare fails: {figures}"))
    }
}

#[test]
#[ignore = "tier 3: needs the corpus (`just corpus`)"]
fn opus_vectors_conform() {
    let vectors = selected();
    let results = each(&vectors, |(n, bit)| run(n, bit));
    let mut failures = vec![];
    for ((n, _), result) in vectors.iter().zip(results) {
        match result {
            Ok(summary) => println!("testvector{n}: passes, {summary}"),
            Err(why) => {
                println!("testvector{n}: FAILS: {why}");
                failures.push(format!("testvector{n}: {why}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Q of `m.dec` against `.dec` per vector (RFC 8251 section 10: without the intensity-stereo
/// phase inversion), measured with this port once. The two sets differ only where CELT codes
/// intensity stereo, and they need not pass against each other (07 and 09 do not).
const M_DEC_Q: [(&str, f32); 12] = [
    ("01", 54.047_493),
    ("02", 100.0),
    ("03", 100.0),
    ("04", 100.0),
    ("05", 49.127_44),
    ("06", 41.677_715),
    ("07", -93.261_01),
    ("08", 5.094_238),
    ("09", -11.767_158_5),
    ("10", 100.0),
    ("11", 97.054_72),
    ("12", 100.0),
];

/// `opus_compare` itself on the references: each against itself gives Q = 100 (err 0), and
/// `.dec` against `m.dec` the figures above, which hold the port of `opus_compare.c` still.
#[test]
#[ignore = "tier 3: needs the corpus (`just corpus`)"]
fn opus_compare_on_the_references() {
    let vectors = selected();
    let results = each(&vectors, |(n, _)| {
        let dec = read_pcm16(&corpus(&format!("testvector{n}.dec")));
        let m = read_pcm16(&corpus(&format!("testvector{n}m.dec")));
        let same = compare_pcm16(&dec, &dec, true, 48000).unwrap();
        let other = compare_pcm16(&dec, &m, true, 48000).unwrap();
        (same, other)
    });
    for ((n, _), (same, other)) in vectors.iter().zip(results) {
        println!("testvector{n}: m.dec Q {} err {}", other.q, other.err);
        assert_eq!((same.q, same.err), (100.0, 0.0), "testvector{n}");
        let (_, want) = M_DEC_Q.iter().find(|(v, _)| v == n).unwrap();
        assert!(
            (other.q - want).abs() < 1e-3,
            "testvector{n}: {other:?}, frozen {want}"
        );
    }
}
