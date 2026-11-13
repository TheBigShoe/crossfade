//! Xiph's Vorbis test files, decoded packet by packet by this crate and by lewton (the oracle),
//! compared sample by sample. Ignored: the files are not in the repository (Xiph states no
//! licence for them). Run with
//! `CROSSFADE_VORBIS_XIPH_DIR=<dir of *.ogg> cargo test -p crossfade-vorbis --release --test lewton_oracle -- --ignored --nocapture`.
//!
//! lewton returns, per packet, everything that is final (up to where the next window's slope
//! starts), where this crate returns from centre to centre as libvorbis does: the two streams
//! hold the same samples at the same times, split differently. lewton starts later when the
//! first block is long and the second short, by (long - short) / 4 frames, and runs on past
//! the last centre; the comparison covers what both hold.

use crossfade_vorbis::{Comments, Decoder, Ident, Setup};
use lewton::audio::{PreviousWindowRight, read_audio_packet_generic};
use lewton::header::{read_header_ident, read_header_setup};
use lewton::samples::InterleavedSamples;
use ogg::reading::PacketReader;
use std::path::Path;

struct Link {
    packets: Vec<Vec<u8>>,
}

/// The logical streams of a file, one after another (a chained file has several).
fn links(bytes: &[u8]) -> Vec<Link> {
    let mut rd = PacketReader::new(std::io::Cursor::new(bytes.to_vec()));
    let mut out: Vec<(u32, Link)> = vec![];
    while let Ok(Some(p)) = rd.read_packet() {
        let serial = p.stream_serial();
        if p.first_in_stream() || !out.iter().any(|(s, _)| *s == serial) {
            out.push((serial, Link { packets: vec![] }));
        }
        if let Some((_, l)) = out.iter_mut().rev().find(|(s, _)| *s == serial) {
            l.packets.push(p.data);
        }
    }
    out.into_iter().map(|(_, l)| l).collect()
}

#[derive(Default)]
struct Outcome {
    packets: usize,
    ours: Vec<f32>,
    ours_errors: Vec<usize>,
    theirs: Vec<f32>,
    their_errors: Vec<usize>,
    skip: usize,
    channels: usize,
}

fn decode(link: &Link) -> Result<Outcome, String> {
    let [id, comment, setup, audio @ ..] = &link.packets[..] else {
        return Err("fewer than three packets".into());
    };
    let ident = Ident::parse(id).map_err(|e| e.to_string())?;
    Comments::parse(comment, 1 << 20).map_err(|e| e.to_string())?;
    let parsed = Setup::parse(setup, &ident, 256 << 20).map_err(|e| e.to_string())?;
    let modes = parsed.modes().clone();
    let mut ours = Decoder::new(&ident, parsed).map_err(|e| e.to_string())?;

    let lid = read_header_ident(id).map_err(|e| format!("lewton ident: {e:?}"))?;
    let lsetup = read_header_setup(
        setup,
        lid.audio_channels,
        (lid.blocksize_0, lid.blocksize_1),
    )
    .map_err(|e| format!("lewton setup: {e:?}"))?;
    let mut pwr = PreviousWindowRight::new();

    let mut o = Outcome {
        channels: usize::from(ident.channels),
        ..Outcome::default()
    };
    let mut sizes = vec![];
    for (i, p) in audio.iter().enumerate() {
        o.packets += 1;
        match ours.decode(p, &mut o.ours) {
            Ok(_) => {
                if sizes.len() < 2 {
                    sizes.push(modes.block_size(p).expect("decoded, so it has a mode"));
                }
            }
            Err(_) => o.ours_errors.push(i),
        }
        match read_audio_packet_generic::<InterleavedSamples<f32>>(&lid, &lsetup, p, &mut pwr) {
            Ok(d) => o.theirs.extend_from_slice(&d.samples),
            Err(_) => o.their_errors.push(i),
        }
    }
    if let [a, b] = sizes[..]
        && a > b
    {
        o.skip = (a - b) / 4 * o.channels;
    }
    Ok(o)
}

#[test]
#[ignore = "needs Xiph's Vorbis test files: set CROSSFADE_VORBIS_XIPH_DIR"]
fn xiph_files_agree_with_lewton() {
    let dir = std::env::var("CROSSFADE_VORBIS_XIPH_DIR")
        .expect("CROSSFADE_VORBIS_XIPH_DIR names the directory of Xiph's .ogg files");
    let mut files: Vec<_> = std::fs::read_dir(Path::new(&dir))
        .expect("read the directory")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "ogg"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no .ogg files in {dir}");
    println!(
        "{:<28} {:>4} {:>7} {:>10} {:>10} {:>11} {:>11} {:>6}",
        "file#link", "ch", "packets", "frames", "lewton", "max |diff|", "rms diff", "errors"
    );
    let mut worst = 0.0f64;
    let mut failures = vec![];
    for f in &files {
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        let bytes = std::fs::read(f).unwrap();
        for (k, link) in links(&bytes).iter().enumerate() {
            let label = format!("{name}#{k}");
            let o = match decode(link) {
                Ok(o) => o,
                Err(e) => {
                    println!("{label:<28} refused: {e}");
                    failures.push(label);
                    continue;
                }
            };
            let ours = &o.ours[o.skip.min(o.ours.len())..];
            let n = ours.len().min(o.theirs.len());
            let (mut max, mut sq) = (0.0f64, 0.0f64);
            for (a, b) in ours[..n].iter().zip(&o.theirs[..n]) {
                let d = f64::from(*a) - f64::from(*b);
                max = max.max(d.abs());
                sq += d * d;
            }
            let rms = if n > 0 { (sq / n as f64).sqrt() } else { 0.0 };
            worst = worst.max(max);
            let errors = if o.ours_errors == o.their_errors {
                format!("{}", o.ours_errors.len())
            } else {
                failures.push(format!("{label}: errors differ"));
                format!(
                    "ours {:?} lewton {:?}",
                    &o.ours_errors[..o.ours_errors.len().min(5)],
                    &o.their_errors[..o.their_errors.len().min(5)]
                )
            };
            println!(
                "{label:<28} {:>4} {:>7} {:>10} {:>10} {:>11.3e} {:>11.3e} {:>6}",
                o.channels,
                o.packets,
                o.ours.len() / o.channels,
                o.theirs.len() / o.channels,
                max,
                rms,
                errors
            );
            if max > 1e-4 {
                failures.push(format!("{label}: max difference {max:e}"));
            }
        }
    }
    println!("worst max |diff| {worst:e}");
    assert!(failures.is_empty(), "{failures:#?}");
}
