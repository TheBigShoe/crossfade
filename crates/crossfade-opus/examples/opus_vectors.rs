//! The RFC 8251 test vectors, from their TOC bytes alone: what `tests/VECTORS.md` records.
//!
//! ```sh
//! D=.corpus-cache/opus/rfc8251
//! cargo run -p crossfade-opus --example opus_vectors -- $D/testvector*.bit/testvector*.bit
//! ```

use crossfade_opus::testing::bitfile::read_bit_file;
use crossfade_opus::{Bandwidth, Mode, Packet};
use std::collections::BTreeMap;
use std::fmt::Write as _;

#[derive(Default)]
struct Map {
    packets: usize,
    lost: usize,
    invalid: usize,
    samples: usize,
    mode: BTreeMap<&'static str, usize>,
    bandwidth: BTreeMap<&'static str, usize>,
    frame_ms: BTreeMap<u32, usize>,
    stereo: [usize; 2],
    code: [usize; 4],
    mode_switches: usize,
    stereo_switches: usize,
    bandwidth_switches: usize,
    max_frames: usize,
    empty_frames: usize,
    one_byte_frames: usize,
    padded: usize,
    max_padding: usize,
    no_range: usize,
}

fn mode_name(m: Mode) -> &'static str {
    match m {
        Mode::Silk => "SILK",
        Mode::Hybrid => "Hybrid",
        Mode::Celt => "CELT",
    }
}

fn bandwidth_name(b: Bandwidth) -> &'static str {
    match b {
        Bandwidth::Narrow => "NB",
        Bandwidth::Medium => "MB",
        Bandwidth::Wide => "WB",
        Bandwidth::SuperWide => "SWB",
        Bandwidth::Full => "FB",
    }
}

fn map(bytes: &[u8]) -> Result<Map, String> {
    let mut m = Map::default();
    let mut prev: Option<crossfade_opus::Toc> = None;
    let mut last_duration = 0;
    for p in read_bit_file(bytes)? {
        m.packets += 1;
        m.no_range += usize::from(p.final_range == 0);
        if p.data.is_empty() {
            m.lost += 1;
            m.samples += last_duration;
            continue;
        }
        let Ok(packet) = Packet::parse(p.data) else {
            m.invalid += 1;
            continue;
        };
        let toc = packet.toc();
        last_duration = packet.duration();
        m.samples += last_duration;
        *m.mode.entry(mode_name(toc.mode())).or_default() += 1;
        *m.bandwidth
            .entry(bandwidth_name(toc.bandwidth()))
            .or_default() += 1;
        *m.frame_ms
            .entry(toc.frame_samples() as u32 * 10 / 48)
            .or_default() += 1;
        m.stereo[usize::from(toc.stereo())] += 1;
        m.code[usize::from(toc.code())] += 1;
        if let Some(prev) = prev {
            m.mode_switches += usize::from(prev.mode() != toc.mode());
            m.stereo_switches += usize::from(prev.stereo() != toc.stereo());
            m.bandwidth_switches += usize::from(prev.bandwidth() != toc.bandwidth());
        }
        prev = Some(toc);
        m.max_frames = m.max_frames.max(packet.frames().len());
        m.empty_frames += packet.frames().iter().filter(|f| f.is_empty()).count();
        m.one_byte_frames += packet.frames().iter().filter(|f| f.len() == 1).count();
        if toc.code() == 3 && p.data[1] & 0x40 != 0 {
            m.padded += 1;
            m.max_padding = m.max_padding.max(packet.padding().len());
        }
    }
    Ok(m)
}

fn counts<K: std::fmt::Display>(c: &BTreeMap<K, usize>, unit: &str) -> String {
    c.iter()
        .map(|(k, n)| format!("{k}{unit} {n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn main() {
    let mut table = String::from(
        "| Vector | Packets | Duration | Mode | Bandwidth | Frame | Mono/stereo | Code 0/1/2/3 | Switches (mode/stereo/bw) |\n|---|---|---|---|---|---|---|---|---|\n",
    );
    let mut notes = String::new();
    for path in std::env::args().skip(1) {
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let m = map(&bytes).unwrap_or_else(|e| panic!("{path}: {e}"));
        let name = std::path::Path::new(&path)
            .file_stem()
            .map_or(path.clone(), |s| s.to_string_lossy().into_owned());
        let _ = writeln!(
            table,
            "| {name} | {} | {:.2} s | {} | {} | {} | {} / {} | {} / {} / {} / {} | {} / {} / {} |",
            m.packets,
            m.samples as f64 / 48_000.0,
            counts(&m.mode, ""),
            counts(&m.bandwidth, ""),
            counts(
                &m.frame_ms
                    .iter()
                    .map(|(k, n)| (format!("{}", f64::from(*k) / 10.0), *n))
                    .collect(),
                " ms"
            ),
            m.stereo[0],
            m.stereo[1],
            m.code[0],
            m.code[1],
            m.code[2],
            m.code[3],
            m.mode_switches,
            m.stereo_switches,
            m.bandwidth_switches,
        );
        let _ = writeln!(
            notes,
            "| {name} | {} | {} | {} | {} | {} | {} | {} | {} |",
            m.lost,
            m.invalid,
            m.max_frames,
            m.empty_frames,
            m.one_byte_frames,
            m.padded,
            m.max_padding,
            m.no_range,
        );
    }
    print!(
        "{table}\n| Vector | Lost (length 0) | Invalid | Most frames in a packet | Empty frames | 1-byte frames | Padded packets | Most padding | Final range 0 |\n|---|---|---|---|---|---|---|---|---|\n{notes}"
    );
}
