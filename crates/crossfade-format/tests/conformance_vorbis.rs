//! Tier 3: Xiph's 29 Vorbis test files (`vorbis/xiph/*` in `corpus.toml`) through the Ogg
//! reader and the engine's Vorbis decoder. `#[ignore]`d so a plain `cargo test` stays offline;
//! `just conformance` runs them and they fail, never skip, when a file is missing.
//!
//! Each file's packet count, timing, decoded frames and a digest of the `f32` output are held
//! to the frozen `tests/conformance/vorbis.txt` (`CROSSFADE_BLESS=1` rewrites it), and the
//! output is compared with lewton's decode of the same packets.

use crossfade_codec::decoder_for;
use crossfade_core::{AudioFrame, Limits, Packet, Samples};
use crossfade_corpus::require_all;
use lewton::audio::{PreviousWindowRight, read_audio_packet_generic};
use lewton::header::{read_header_ident, read_header_setup};
use lewton::samples::InterleavedSamples;
use sha2::{Digest, Sha256};
use std::path::Path;

const FROZEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/conformance/vorbis.txt");

/// The worst difference measured against lewton on these files was 8.8e-6 (make.ogg), within
/// float rounding: about a third of a 16-bit step.
const LEWTON_TOLERANCE: f32 = 2e-5;

struct Run {
    packets: u64,
    first_pts: i64,
    /// Decoded frames (priming and padding included) and the stream's audible frames.
    decoded: u64,
    declared: Option<u64>,
    sha256: String,
    /// Largest |difference| from lewton, and the frames compared.
    lewton: (f32, usize),
}

fn run(path: &Path) -> Result<Run, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let (_, mut demux) = crossfade_format::open_demuxer(Box::new(file), &Limits::default())
        .map_err(|e| e.to_string())?;
    let stream = demux.streams()[0].clone();
    let ch = usize::from(stream.audio.layout().channels());
    let mut dec = decoder_for(&stream).map_err(|e| e.to_string())?;
    let [id, _, setup] = crossfade_vorbis::unpack_headers(&stream.extradata).unwrap();
    let lid = read_header_ident(id).unwrap();
    let lsetup = read_header_setup(
        setup,
        lid.audio_channels,
        (lid.blocksize_0, lid.blocksize_1),
    )
    .unwrap();
    let mut pwr = PreviousWindowRight::new();
    let mut theirs: Vec<f32> = vec![];
    let mut ours: Vec<f32> = vec![];
    let mut sha = Sha256::new();
    let mut r = Run {
        packets: 0,
        first_pts: 0,
        decoded: 0,
        declared: stream.frames,
        sha256: String::new(),
        lewton: (0.0, 0),
    };
    let mut p = Packet::default();
    let mut f = AudioFrame::new(&stream.audio);
    let mut sizes = vec![];
    let modes = crossfade_vorbis::Setup::parse(
        setup,
        &crossfade_vorbis::Ident::parse(id).unwrap(),
        usize::MAX,
    )
    .unwrap()
    .modes()
    .clone();
    while demux.read_packet(&mut p).map_err(|e| e.to_string())? {
        if r.packets == 0 {
            r.first_pts = p.pts;
        }
        r.packets += 1;
        if sizes.len() < 2
            && let Ok(n) = modes.block_size(&p.data)
        {
            sizes.push(n);
        }
        dec.send_packet(&p).map_err(|e| e.to_string())?;
        while dec.receive_frame(&mut f).map_err(|e| e.to_string())? {
            let Samples::F32(s) = &f.samples else {
                return Err("vorbis decodes to f32".into());
            };
            r.decoded += f.frames() as u64;
            s.iter().for_each(|v| sha.update(v.to_le_bytes()));
            ours.extend_from_slice(s);
        }
        if let Ok(d) =
            read_audio_packet_generic::<InterleavedSamples<f32>>(&lid, &lsetup, &p.data, &mut pwr)
        {
            theirs.extend_from_slice(&d.samples);
        }
    }
    r.sha256 = sha.finalize().iter().map(|b| format!("{b:02x}")).collect();
    // lewton starts later when the first block is long and the second short.
    let skip = match sizes[..] {
        [a, b] if a > b => (a - b) / 4 * ch,
        _ => 0,
    };
    let ours = &ours[skip.min(ours.len())..];
    let n = ours.len().min(theirs.len());
    r.lewton = (
        ours[..n]
            .iter()
            .zip(&theirs[..n])
            .fold(0.0f32, |m, (a, b)| m.max((a - b).abs())),
        n / ch,
    );
    Ok(r)
}

const FROZEN_HEADER: &str = "\
# Tier 3: Xiph's Vorbis test files through the Ogg reader and the Vorbis decoder.
# Rewrite with `CROSSFADE_BLESS=1 just conformance` after a deliberate change.
# id packets first_pts decoded_frames declared_frames sha256(f32 le output)
";

#[test]
#[ignore = "needs the corpus: just corpus, then just conformance"]
fn xiph_vorbis_files_decode_as_frozen_and_agree_with_lewton() {
    let files = require_all("vorbis/xiph/");
    assert_eq!(files.len(), 29, "Xiph's 29 Vorbis files");
    let mut text = String::from(FROZEN_HEADER);
    let mut failures = vec![];
    let mut worst = 0.0f32;
    for (id, path) in &files {
        match run(path) {
            Ok(r) => {
                text.push_str(&format!(
                    "{id} {} {} {} {} {}\n",
                    r.packets,
                    r.first_pts,
                    r.decoded,
                    r.declared.map_or("-".into(), |d| d.to_string()),
                    r.sha256
                ));
                worst = worst.max(r.lewton.0);
                if r.lewton.0 > LEWTON_TOLERANCE || r.lewton.1 == 0 {
                    failures.push(format!(
                        "{id}: differs from lewton by {} over {} frames",
                        r.lewton.0, r.lewton.1
                    ));
                }
            }
            Err(e) => {
                text.push_str(&format!("{id} error {e}\n"));
                failures.push(format!("{id}: {e}"));
            }
        }
    }
    println!("worst difference from lewton: {worst:e}");
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
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
