//! Tier 3: the ISO/IEC 14496-26 AAC streams of the corpus (`aac/iso/`) through the readers
//! (MP4, ADTS) and the AAC decoder, against the ISO reference `.wav` decodes. Built only with
//! the `aac` feature (`cargo test -p crossfade-format --features aac`); `#[ignore]`d like the
//! other conformance tests, so `just conformance` runs it and a missing file fails it.
//!
//! Each stream is decoded, trimmed to its timeline as the engine does (frames before pts 0
//! dropped, cut at `frames`), aligned with its reference (the ISO decoder's output starts where
//! it starts: a few known offsets are tried) and held to the ISO full-accuracy bounds, an RMS
//! difference under 2^-15/sqrt(12) and a largest difference under 2^-14 of full scale.
//! Streams with noise substitution (al18, al19) cannot meet a sample-wise bound (the noise is
//! specified only by its energy) and are held to their energy instead: every 2048-frame block's
//! energy within 3 dB of the reference's and the whole within 0.1 dB. A stream using a tool the
//! decoder refuses by name (960-sample frames, coupling channels, LTP) is recorded as refused;
//! ADIF has no reader. Streams whose references come one file per channel are decoded and
//! recorded without a comparison. Everything measured is frozen in
//! `tests/conformance/aac.txt` (`CROSSFADE_BLESS=1` rewrites it), so a change is noticed.
#![cfg(feature = "aac")]

use crossfade_codec::decoder_for;
use crossfade_core::{AudioFrame, Error, Limits, Packet, Samples};
use crossfade_corpus::require_all;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::path::Path;

const FROZEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/conformance/aac.txt");
const MAX_RMS: f64 = 8.8e-6;
const MAX_DIFF: f64 = 1.0 / 16384.0;
/// Streams that use perceptual noise substitution.
const PNS: [&str; 2] = ["al18", "al19"];

/// Decodes `path` through `open_demuxer` and the engine's decoder, trimmed to the timeline.
/// Interleaved samples and the channel count.
fn decode(path: &Path) -> Result<(Vec<f32>, usize), Error> {
    let file = std::fs::File::open(path)?;
    let (_, mut demux) = crossfade_format::open_demuxer(Box::new(file), &Limits::default())?;
    let stream = demux.streams()[0].clone();
    let channels = usize::from(stream.audio.layout().channels());
    let mut decoder = decoder_for(&stream)?;
    let (mut packet, mut frame) = (Packet::default(), AudioFrame::new(&stream.audio));
    let end = stream.frames.map_or(i64::MAX, |f| f as i64);
    let mut out = Vec::new();
    let mut take = |f: &AudioFrame| {
        let Samples::F32(v) = &f.samples else {
            panic!("aac yields f32")
        };
        for (i, s) in v.chunks_exact(channels).enumerate() {
            let pts = f.pts + i as i64;
            if (0..end).contains(&pts) {
                out.extend_from_slice(s);
            }
        }
    };
    while demux.read_packet(&mut packet)? {
        decoder.send_packet(&packet)?;
        while decoder.receive_frame(&mut frame)? {
            take(&frame);
        }
    }
    decoder.drain();
    while decoder.receive_frame(&mut frame)? {
        take(&frame);
    }
    Ok((out, channels))
}

/// The reference decode: interleaved, scaled to -1..1.
fn reference(path: &Path) -> (Vec<f64>, usize) {
    let file = std::fs::File::open(path).unwrap();
    let (_, mut demux) =
        crossfade_format::open_demuxer(Box::new(file), &Limits::default()).unwrap();
    let stream = demux.streams()[0].clone();
    let channels = usize::from(stream.audio.layout().channels());
    let mut decoder = decoder_for(&stream).unwrap();
    let (mut packet, mut frame) = (Packet::default(), AudioFrame::new(&stream.audio));
    let mut out = Vec::new();
    while demux.read_packet(&mut packet).unwrap() {
        decoder.send_packet(&packet).unwrap();
        while decoder.receive_frame(&mut frame).unwrap() {
            match &frame.samples {
                Samples::S16(v) => out.extend(v.iter().map(|&s| f64::from(s) / 32768.0)),
                Samples::S24(v) => out.extend(v.iter().map(|&s| f64::from(s) / 8_388_608.0)),
                other => panic!("reference in {:?}", other.format()),
            }
        }
    }
    (out, channels)
}

struct Compared {
    offset: i64,
    rms: f64,
    max: f64,
    /// Energy ratio in dB over all, and the largest per 2048-frame block (both absolute).
    energy_db: f64,
    block_db: f64,
}

/// Our frame `i + offset` against reference frame `i`, where both exist.
fn compare(ours: &[f32], reference: &[f64], channels: usize, offset: i64) -> Compared {
    let (n_ours, n_ref) = (ours.len() / channels, reference.len() / channels);
    let (mut sum, mut n, mut max) = (0.0, 0usize, 0.0f64);
    let (mut e_ours, mut e_ref) = (0.0, 0.0);
    let mut block_db = 0.0f64;
    let (mut b_ours, mut b_ref) = (0.0, 0.0);
    for i in 0..n_ref {
        let j = i as i64 + offset;
        if j < 0 || j as usize >= n_ours {
            continue;
        }
        for c in 0..channels {
            let (g, w) = (
                f64::from(ours[j as usize * channels + c]),
                reference[i * channels + c],
            );
            let d = g - w;
            sum += d * d;
            n += 1;
            max = max.max(d.abs());
            b_ours += g * g;
            b_ref += w * w;
        }
        if (i + 1) % 2048 == 0 {
            // Blocks quieter than -60 dBFS say nothing about the noise's level.
            if b_ref / (2048.0 * channels as f64) > 1e-6 {
                block_db = block_db.max((10.0 * (b_ours / b_ref).log10()).abs());
            }
            (e_ours, e_ref) = (e_ours + b_ours, e_ref + b_ref);
            (b_ours, b_ref) = (0.0, 0.0);
        }
    }
    Compared {
        offset,
        rms: (sum / n.max(1) as f64).sqrt(),
        max,
        energy_db: (10.0 * (e_ours / e_ref.max(1e-30)).log10()).abs(),
        block_db,
    }
}

/// One line of the frozen results.
fn result(id: &str, path: &Path, references: &[(String, std::path::PathBuf)]) -> (String, bool) {
    let name = id.rsplit('/').next().unwrap_or(id);
    let stem = name.split('.').next().unwrap_or(name);
    let decoded = match decode(path) {
        Ok(d) => d,
        Err(Error::Unsupported { what }) => return (format!("{id} refused: {what}"), true),
        Err(Error::UnrecognisedFormat) if name.ends_with(".adif") => {
            return (format!("{id} refused: ADIF has no reader"), true);
        }
        Err(e) => return (format!("{id} FAILED: {e}"), false),
    };
    let (ours, channels) = decoded;
    let mut sha = Sha256::new();
    for s in &ours {
        sha.update(s.to_bits().to_le_bytes());
    }
    let digest: String = sha.finalize()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let frames = ours.len() / channels;
    let reference_id = format!(
        "aac/iso/{}.wav",
        stem.split('_').take(2).collect::<Vec<_>>().join("_")
    );
    let Some((_, ref_path)) = references.iter().find(|(r, _)| *r == reference_id) else {
        return (
            format!(
                "{id} decoded: {channels} ch, {frames} frames, sha256 {digest} (no reference in the corpus)"
            ),
            true,
        );
    };
    let (want, ref_channels) = reference(ref_path);
    assert_eq!(ref_channels, channels, "{id}");
    let pns = PNS.iter().any(|p| stem.starts_with(p));
    // Aligned where the samples match best; with noise substitution, where the block energies do.
    let best = [0i64, 2048, 1024, -1024, -2048, 2112, 960, 1600]
        .into_iter()
        .map(|o| compare(&ours, &want, channels, o))
        .min_by(|a, b| {
            if pns {
                a.block_db.total_cmp(&b.block_db)
            } else {
                a.rms.total_cmp(&b.rms)
            }
        })
        .unwrap_or_else(|| unreachable!());
    let (verdict, ok) = if pns {
        let ok = best.energy_db < 0.1 && best.block_db < 3.0;
        (
            if ok {
                "pass (energy)"
            } else {
                "FAILED (energy)"
            },
            ok,
        )
    } else {
        let ok = best.rms < MAX_RMS && best.max < MAX_DIFF;
        (if ok { "pass" } else { "FAILED" }, ok)
    };
    let mut line = format!(
        "{id} {verdict}: {channels} ch, {frames} frames, offset {}",
        best.offset
    );
    write!(
        line,
        ", rms {:.2e}, max {:.2e}, energy {:.3} dB, worst block {:.2} dB, sha256 {digest}",
        best.rms, best.max, best.energy_db, best.block_db
    )
    .unwrap_or_default();
    (line, ok)
}

#[test]
#[ignore = "needs the corpus: `just corpus`, then `just conformance`"]
fn iso_aac_streams_meet_full_accuracy_and_match_the_frozen_results() {
    let all = require_all("aac/iso/");
    let references: Vec<_> = all
        .iter()
        .filter(|(id, _)| id.ends_with(".wav"))
        .cloned()
        .collect();
    let mut text = String::from(
        "# ISO/IEC 14496-26 AAC streams through the readers and the AAC decoder.\n\
         # Rewrite with `CROSSFADE_BLESS=1 just conformance` after a deliberate decoder change.\n",
    );
    let mut failed = Vec::new();
    for (id, path) in all.iter().filter(|(id, _)| !id.ends_with(".wav")) {
        let (line, ok) = result(id, path, &references);
        if !ok {
            failed.push(line.clone());
        }
        text.push_str(&line);
        text.push('\n');
    }
    if std::env::var_os("CROSSFADE_BLESS").is_some() {
        std::fs::write(FROZEN, &text).expect("write the frozen results");
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    let frozen = std::fs::read_to_string(FROZEN)
        .unwrap_or_else(|e| panic!("{FROZEN}: {e}; create it with CROSSFADE_BLESS=1"));
    let differ: Vec<_> = frozen
        .lines()
        .zip(text.lines())
        .filter(|(f, n)| f != n)
        .map(|(f, n)| format!("frozen: {f}\n   now: {n}"))
        .collect();
    assert!(
        differ.is_empty() && frozen.lines().count() == text.lines().count(),
        "frozen results have {} lines, this run {}; differing:\n{}",
        frozen.lines().count(),
        text.lines().count(),
        differ.join("\n")
    );
}
