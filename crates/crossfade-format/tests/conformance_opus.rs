//! Tier 3: the RFC 8251 Opus test vectors (`opus/rfc8251/*` in `corpus.toml`) through
//! `crossfade_opus::Decoder` at 48 kHz stereo, as `run_vectors.sh` runs `opus_demo -d 48000 2`
//! and `opus_compare -s`. `#[ignore]`d so a plain `cargo test` stays offline; `just
//! conformance` runs them and they fail, never skip, when a file is missing.
//!
//! For every packet the decoder's final range must equal the encoder's stored in the `.bit`
//! file (the strict test of the entropy decoding: the first mismatch is reported with its
//! packet index and TOC); then the output must pass `opus_compare` against `.dec` or `m.dec`
//! (RFC 8251 section 11: either set).
//!
//! The SILK-only vectors (02, 03, 04) must equal `.dec` sample for sample.
//!
//! Per vector the packet count, "range ok" and Q against each set (one decimal) are held to
//! the frozen `tests/conformance/opus.txt` (`CROSSFADE_BLESS=1` rewrites it); a run that
//! selects vectors is not compared with it.
//!
//! `OPUS_VECTORS=01,07` runs only those vectors; `OPUS_MODES=celt` (or `silk`, `hybrid`, a
//! comma-separated list) only the vectors whose packets all use the modes listed (see
//! `crates/crossfade-opus/tests/VECTORS.md`); `OPUS_PACKETS=1` prints every packet.

use crossfade_opus::testing::bitfile::{read_bit_file, read_pcm16};
use crossfade_opus::testing::compare::compare_pcm16;
use crossfade_opus::testing::vectors::{decode_vector, modes};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

const FROZEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/conformance/opus.txt");

const FROZEN_HEADER: &str = "\
# Tier 3: the RFC 8251 Opus test vectors through crossfade_opus::Decoder at 48 kHz stereo.
# Rewrite with `CROSSFADE_BLESS=1 just conformance` after a deliberate change.
# vector packets final-range Q-against-.dec Q-against-m.dec
";

const SAMPLE_EXACT: [&str; 3] = ["02", "03", "04"];

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

/// `Ok(frozen line)` when the vector passes, `Err(why)` otherwise.
fn run(n: &str, bit: &[u8]) -> Result<String, String> {
    let packets = read_bit_file(bit)?.len();
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
        // SILK alone is integer arithmetic: the SILK-only vectors are sample-exact.
        if SAMPLE_EXACT.contains(&n) && suffix == ".dec" && reference != pcm {
            return Err("a SILK-only vector is not sample-exact".into());
        }
        let q = compare_pcm16(&reference, &pcm, true, 48000)?;
        passes |= q.passes();
        figures.push(format!("{:.1}", q.q));
    }
    let line = format!("testvector{n} {packets} range-ok {}", figures.join(" "));
    if passes {
        Ok(line)
    } else {
        Err(format!("opus_compare fails: {line}"))
    }
}

#[test]
#[ignore = "tier 3: needs the corpus (`just corpus`)"]
fn opus_vectors_conform() {
    let vectors = selected();
    let results = each(&vectors, |(n, bit)| run(n, bit));
    let mut failures = vec![];
    let mut text = String::from(FROZEN_HEADER);
    for ((n, _), result) in vectors.iter().zip(results) {
        match result {
            Ok(line) => {
                println!("{line}");
                text.push_str(&line);
                text.push('\n');
            }
            Err(why) => {
                println!("testvector{n}: FAILS: {why}");
                text.push_str(&format!("testvector{n} error {why}\n"));
                failures.push(format!("testvector{n}: {why}"));
            }
        }
    }
    if vectors.len() == VECTORS.len() {
        if std::env::var_os("CROSSFADE_BLESS").is_some() {
            std::fs::write(FROZEN, &text).expect("write the frozen results");
        }
        let frozen = std::fs::read_to_string(FROZEN)
            .unwrap_or_else(|e| panic!("{FROZEN}: {e}; create it with CROSSFADE_BLESS=1"));
        for (want, got) in frozen.lines().zip(text.lines()) {
            if want != got {
                failures.push(format!("frozen: {want}\n   now: {got}"));
            }
        }
        if frozen.lines().count() != text.lines().count() {
            failures.push("a different number of results than frozen".into());
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

/// Pre-skip of the Ogg wrapping: what libopus's encoder writes at 48 kHz.
const PRE_SKIP: usize = 312;

/// After a seek the decoder starts afresh 1 s early (RFC 7845 4.6 asks for at least 80 ms);
/// it has not always converged exactly by the target. Measured through the engine on these
/// vectors: 0 for the CELT-only, SILK-only NB/MB and 08/09, at most 9.155e-4 (30 16-bit steps,
/// testvector10); see also the `opus_seek_convergence` example.
const SEEK_TOLERANCE: f32 = 1e-3;

/// The reader's pre-roll, which every target but the first has in full before it.
const PRE_ROLL: i64 = 48_000;

/// Every frame's samples through the Ogg reader and the engine's adapter, from where the reader
/// stands, until `until` (a pts) is covered: `(pts of the first, samples)`.
fn engine_decode(
    demux: &mut dyn crossfade_core::Demuxer,
    dec: &mut dyn crossfade_core::Decoder,
    until: i64,
) -> (i64, Vec<f32>) {
    use crossfade_core::{AudioFrame, Packet, Samples};
    let mut out = vec![];
    let mut first = None;
    let mut p = Packet::default();
    let mut f = AudioFrame::new(&demux.streams()[0].audio);
    while demux.read_packet(&mut p).unwrap() {
        dec.send_packet(&p).unwrap();
        while dec.receive_frame(&mut f).unwrap() {
            let Samples::F32(s) = &f.samples else {
                panic!("opus decodes to f32")
            };
            let start = *first.get_or_insert(f.pts);
            assert_eq!(f.pts, start + (out.len() / 2) as i64, "frames follow on");
            out.extend_from_slice(s);
        }
        if first.is_some_and(|s| s + (out.len() / 2) as i64 >= until) {
            break;
        }
    }
    (first.unwrap_or(0), out)
}

/// One vector wrapped into Ogg Opus in memory decodes through the reader and the adapter to
/// the direct decode's samples; seeks land early enough to be within [`SEEK_TOLERANCE`].
/// Returns the worst difference after a seek.
fn through_ogg(bit: &[u8]) -> Result<f32, String> {
    use crossfade_opus::testing::ogg::{OggLayout, ogg_file, opus_head, opus_tags};
    let packets: Vec<Vec<u8>> = read_bit_file(bit)?
        .iter()
        .map(|p| p.data.to_vec())
        .collect();
    let mut direct = vec![];
    let mut d = crossfade_opus::Decoder::new(2).map_err(|e| e.to_string())?;
    for p in &packets {
        d.decode(Some(p), &mut direct).map_err(|e| e.to_string())?;
    }
    let bytes = ogg_file(
        &opus_head(2, PRE_SKIP as u16, 0, None),
        &opus_tags("crossfade conformance", &[]),
        &packets,
        &OggLayout {
            packets_per_page: vec![3, 1, 4],
            ..OggLayout::default()
        },
    );
    let open = || {
        crossfade_format::open_demuxer(
            Box::new(std::io::Cursor::new(bytes.clone())),
            &crossfade_core::Limits::default(),
        )
        .map(|(_, d)| d)
        .map_err(|e| e.to_string())
    };
    let mut demux = open()?;
    let total = (direct.len() / 2 - PRE_SKIP) as i64;
    if demux.streams()[0].frames != Some(total as u64) {
        return Err(format!(
            "frames {:?}, want {total}",
            demux.streams()[0].frames
        ));
    }
    let mut dec = crossfade_codec::decoder_for(&demux.streams()[0]).map_err(|e| e.to_string())?;
    let (first, all) = engine_decode(&mut *demux, &mut *dec, i64::MAX);
    if first != -(PRE_SKIP as i64) || all != direct {
        return Err("the Ogg wrapping does not decode to the direct decode".into());
    }
    let mut worst = 0.0f32;
    for k in 0..12 {
        let target = total * k / 12 + 1234 * k;
        let landed = demux.seek(target).map_err(|e| e.to_string())?;
        if landed > target {
            return Err(format!("seek to {target} landed at {landed}"));
        }
        if k > 0 && target - landed < PRE_ROLL {
            return Err(format!(
                "seek to {target} landed at {landed}, under 1 s before"
            ));
        }
        dec.reset();
        let (from, got) = engine_decode(&mut *demux, &mut *dec, target + 24_000);
        let skip = (target - from) as usize * 2;
        let at = (target + PRE_SKIP as i64) as usize * 2;
        for (a, b) in got[skip..].iter().zip(&direct[at..]) {
            worst = worst.max((a - b).abs());
        }
    }
    if worst > SEEK_TOLERANCE {
        return Err(format!("a seek is {worst} off the decode from the start"));
    }
    Ok(worst)
}

#[test]
#[ignore = "tier 3: needs the corpus (`just corpus`)"]
fn the_vectors_wrapped_in_ogg_decode_and_seek_through_the_engine() {
    let vectors = selected();
    let results = each(&vectors, |(_, bit)| through_ogg(bit));
    let mut failures = vec![];
    for ((n, _), r) in vectors.iter().zip(results) {
        match r {
            Ok(worst) => println!("testvector{n}: through Ogg, worst after a seek {worst:.3e}"),
            Err(e) => failures.push(format!("testvector{n}: {e}")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
