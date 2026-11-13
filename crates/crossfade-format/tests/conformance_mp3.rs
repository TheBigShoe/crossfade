//! Tier 3: ISO/IEC 11172-4 accuracy of the MP3 reader and decoder on the Layer III conformance
//! streams of ISO/IEC 14496-26 (corpus ids `mp3/iso/l3_*.mpg`, with 24-bit `mp3/iso/l3_*.wav`
//! references; ISO's terms allow testing with them and forbid redistributing them, so only the
//! corpus manifest names them). `#[ignore]`d like the rest of the tier; `just conformance` runs
//! it, and it fails (never skips) when a file is missing.
//!
//! Everything is keyed by file name. Each stream goes through `open_demuxer` and the engine's
//! decoder (`decoder_for`), untrimmed: the references start at the decoder's first output
//! sample (they include its delay), so they are compared from offset 0. Two adjustments
//! follow what the references are:
//! - Frames whose main data starts before the file (l3_10204, cut from a longer stream) are
//!   silence in our output, which keeps the timeline; the references leave them out, so do we.
//! - l3_10201 switches between mono and stereo, which the reader refuses at the switch (the
//!   engine's stream description is fixed), naming the byte; its references are one per
//!   segment. Each later segment is opened with the reader from that byte, and its packets are
//!   decoded by `crossfade_mp3::Decoder` itself, fed every packet since the start of the file,
//!   since the first frames after the switch take main data from the frames before it, which
//!   a fresh engine decoder never sees. (The crate's decoder handles the switch.)
//!
//! Criterion, per channel: RMS of the difference below 2^-15/sqrt(12) and largest difference
//! at most 2^-14 of full scale is "full accuracy"; RMS below 2^-11/sqrt(12) is "limited". Every
//! pair must be full, and the verdicts with the measured RMS and largest difference are
//! compared with the frozen `tests/conformance/mp3.txt` (`CROSSFADE_BLESS=1` rewrites it), so a
//! change in the decoder's numbers is noticed even while it stays full.

use crossfade_codec::decoder_for;
use crossfade_core::{AudioFrame, Error, Limits, Packet, Samples};
use crossfade_format::open_demuxer;
use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};

/// (stream, reference, channel of the stream the reference holds: `None` for all).
fn pairs(names: &[String]) -> Vec<(String, String, Option<usize>)> {
    let mut out = Vec::new();
    for mpg in names
        .iter()
        .filter(|n| n.starts_with("l3_") && n.ends_with(".mpg"))
    {
        let stem = mpg.trim_end_matches(".mpg");
        let refs: Vec<(&str, Option<usize>)> = match stem {
            "l3_10201" => vec![
                ("l3_10201_0", None),
                ("l3_10201_1_AA", None),
                ("l3_10201_2", None),
            ],
            "l3_10501_b01+b02" | "l3_10501_b01_b02" => {
                vec![("l3_10501_b00", Some(0)), ("l3_10501_b01", Some(1))]
            }
            "l3_10501_f01+f02" => vec![("l3_10501_f01", Some(0)), ("l3_10501_f02", Some(1))],
            s => vec![(s, None)],
        };
        for (r, ch) in refs {
            if names.contains(&format!("{r}.wav")) {
                out.push((mpg.clone(), format!("{r}.wav"), ch));
            }
        }
    }
    out
}

/// Per channel samples of every segment the reader gives (one unless the stream switches
/// between mono and stereo): the engine decoder's for the first, the crate decoder's (fed the
/// whole file in order) for the later ones.
fn decode(bytes: &[u8]) -> Vec<Vec<Vec<f32>>> {
    let mut segments = Vec::new();
    let mut whole = crossfade_mp3::Decoder::new();
    let mut start = 0usize;
    loop {
        let src = Box::new(Cursor::new(bytes[start..].to_vec()));
        let (_, mut d) = open_demuxer(src, &Limits::default()).unwrap();
        let stream = d.streams()[0].clone();
        let ch = usize::from(stream.audio.layout().channels());
        let mut dec = decoder_for(&stream).unwrap();
        let mut scratch = Vec::new();
        let mut leading = true;
        let mut out = vec![Vec::new(); ch];
        let mut p = Packet::default();
        let mut f = AudioFrame::new(&stream.audio);
        let next = loop {
            match d.read_packet(&mut p) {
                Ok(true) => {}
                Ok(false) => break None,
                Err(Error::Unsupported { what }) if what.contains("changes at byte") => {
                    let at = what
                        .split("byte ")
                        .nth(1)
                        .unwrap()
                        .split(',')
                        .next()
                        .unwrap();
                    break Some(start + at.parse::<usize>().unwrap());
                }
                Err(e) => panic!("{e}"),
            }
            scratch.clear();
            let result = whole.decode_frame(&p.data, &mut scratch);
            // The references leave out the frames whose main data precedes the file.
            let skipped = leading && result.is_err_and(|e| e.is_reservoir());
            leading &= skipped;
            if segments.is_empty() {
                dec.send_packet(&p).unwrap();
                while dec.receive_frame(&mut f).unwrap() {
                    let Samples::F32(s) = &f.samples else {
                        unreachable!()
                    };
                    assert_eq!(s, &scratch, "the engine decoder and the crate's agree");
                }
            }
            if !skipped {
                for (i, v) in scratch.iter().enumerate() {
                    out[i % ch].push(*v);
                }
            }
        };
        segments.push(out);
        match next {
            Some(at) => start = at,
            None => return segments,
        }
    }
}

fn read_wav(path: &Path) -> Vec<Vec<f32>> {
    let b = std::fs::read(path).unwrap();
    let u16_at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
    let u32_at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let (mut p, mut ch, mut bits, mut data) = (12, 0usize, 0u16, &b[..0]);
    while p + 8 <= b.len() {
        let size = u32_at(p + 4) as usize;
        match &b[p..p + 4] {
            b"fmt " => {
                ch = usize::from(u16_at(p + 10));
                bits = u16_at(p + 22);
            }
            b"data" => {
                data = &b[p + 8..(p + 8 + size).min(b.len())];
                break;
            }
            _ => {}
        }
        p += 8 + size + size % 2;
    }
    assert_eq!(bits, 24, "{path:?}");
    let mut out = vec![Vec::new(); ch];
    for (i, s) in data.as_chunks::<3>().0.iter().enumerate() {
        let v = i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8;
        out[i % ch].push(v as f32 / 8_388_608.0);
    }
    out
}

/// RMS and largest difference over the samples both have, ours clipped to the reference's range.
fn compare(ours: &[f32], reference: &[f32]) -> (f64, f64, usize) {
    let top = 8_388_607.0 / 8_388_608.0;
    let n = ours.len().min(reference.len());
    let (mut sum, mut max) = (0f64, 0f64);
    for (&a, &b) in ours[..n].iter().zip(&reference[..n]) {
        let d = f64::from(a.clamp(-1.0, top) - b);
        sum += d * d;
        max = max.max(d.abs());
    }
    ((sum / n.max(1) as f64).sqrt(), max, n)
}

const FROZEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/conformance/mp3.txt");

const FROZEN_HEADER: &str = "\
# Frozen results of the MP3 conformance run (tests/conformance_mp3.rs); do not edit by hand.
# Rewrite with `CROSSFADE_BLESS=1 just conformance` after a deliberate decoder change.
# Columns (tab-separated): stream, reference, samples per channel compared, accuracy by the
# ISO/IEC 11172-4 rule (full, limited or FAIL), RMS and largest difference (worst channel).
";

#[test]
#[ignore = "needs the corpus: `just corpus`, then `just conformance`"]
fn iso_layer3_conformance() {
    let files: BTreeMap<String, PathBuf> = crossfade_corpus::require_all("mp3/iso/")
        .into_iter()
        .map(|(id, path)| (id.trim_start_matches("mp3/iso/").to_owned(), path))
        .collect();
    let names: Vec<String> = files.keys().cloned().collect();
    let full_rms = 2f64.powi(-15) / 12f64.sqrt();
    let full_max = 2f64.powi(-14);
    let limited_rms = 2f64.powi(-11) / 12f64.sqrt();
    let mut text = String::from(FROZEN_HEADER);
    let mut failed = Vec::new();
    let mut cache: Option<(String, Vec<Vec<Vec<f32>>>)> = None;
    for (mpg, wav, channel) in pairs(&names) {
        if cache.as_ref().is_none_or(|(name, _)| *name != mpg) {
            cache = Some((mpg.clone(), decode(&std::fs::read(&files[&mpg]).unwrap())));
        }
        let segments = &cache.as_ref().unwrap().1;
        let reference = read_wav(&files[&wav]);
        // l3_10201's references are its segments in order: _0, _1_AA, _2.
        let segment = match wav.as_str() {
            "l3_10201_1_AA.wav" => &segments[1],
            "l3_10201_2.wav" => &segments[2],
            _ => &segments[0],
        };
        let ours: Vec<&Vec<f32>> = match channel {
            Some(c) => vec![&segment[c]],
            None => segment.iter().collect(),
        };
        assert_eq!(ours.len(), reference.len(), "{mpg} / {wav}: channels");
        let (mut rms, mut max, mut n) = (0f64, 0f64, 0);
        for (o, r) in ours.iter().zip(&reference) {
            let (a, b, k) = compare(o, r);
            rms = rms.max(a);
            max = max.max(b);
            n = k;
        }
        let verdict = if rms < full_rms && max <= full_max {
            "full"
        } else if rms < limited_rms {
            "limited"
        } else {
            "FAIL"
        };
        text.push_str(&format!(
            "{mpg}\t{wav}\t{n}\t{verdict}\t{rms:.3e}\t{max:.3e}\n"
        ));
        if verdict != "full" {
            failed.push(format!("{mpg} / {wav}"));
        }
    }
    print!("{text}");
    assert!(failed.is_empty(), "not full accuracy: {failed:?}");
    if std::env::var_os("CROSSFADE_BLESS").is_some() {
        std::fs::write(FROZEN, &text).expect("write the frozen results");
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
        diffs.is_empty() && want.len() == have.len(),
        "the MP3 conformance results changed ({} lines frozen, {} now):\n{}",
        want.len(),
        have.len(),
        diffs.join("\n")
    );
}
