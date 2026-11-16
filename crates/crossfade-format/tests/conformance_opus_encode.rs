//! Tier 3 for the Opus ENCODER: the reference decodings of four RFC 8251 vectors (`.dec`:
//! 48 kHz stereo speech and music) encoded by `crossfade_opus::Encoder` and decoded again.
//! `#[ignore]`d so a plain `cargo test` stays offline; `just conformance` runs it and it
//! fails, never skips, when a file is missing.
//!
//! Every packet must decode to the encoder's final range. Per vector and setting the bytes
//! written and the signal-to-noise ratio of the decoded audio against the input (one decimal)
//! are held to the frozen `tests/conformance/opus_encode.txt` (`CROSSFADE_BLESS=1` rewrites
//! it): the encoder is free to change, but not by accident, and a change shows what it cost
//! or bought. SNR is a blunt figure for a perceptual codec; it is there to catch a break,
//! not to rank encoders.

use crossfade_opus::testing::bitfile::read_pcm16;
use crossfade_opus::{Decoder, Encoder};

const FROZEN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/conformance/opus_encode.txt"
);

const FROZEN_HEADER: &str = "\
# Tier 3: reference decodings of RFC 8251 vectors through crossfade_opus::Encoder and back.
# Rewrite with `CROSSFADE_BLESS=1 just conformance` after a deliberate change.
# vector setting packets bytes snr-dB
";

/// Speech (SILK-coded originally), hybrid, and two of music.
const VECTORS: [&str; 4] = ["01", "04", "06", "11"];

/// (name, stereo, bits per second, variable).
const SETTINGS: [(&str, bool, u32, bool); 4] = [
    ("stereo-cbr-128k", true, 128_000, false),
    ("stereo-vbr-96k", true, 96_000, true),
    ("mono-cbr-48k", false, 48_000, false),
    ("mono-vbr-24k", false, 24_000, true),
];

/// At most this much of each vector: the tier's time is the decoder vectors'.
const SECONDS: usize = 10;

fn run(
    pcm: &[f32],
    channels: usize,
    bitrate: u32,
    vbr: bool,
) -> Result<(usize, usize, f64), String> {
    let mut enc = Encoder::new(channels).map_err(|e| e.to_string())?;
    enc.set_bitrate(bitrate).map_err(|e| e.to_string())?;
    enc.set_vbr(vbr);
    let mut dec = Decoder::new(channels).map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(pcm.len());
    let (mut packets, mut bytes) = (0, 0);
    let mut packet = Vec::new();
    for chunk in pcm.chunks_exact(960 * channels) {
        packet.clear();
        enc.encode(chunk, &mut packet).map_err(|e| e.to_string())?;
        dec.decode(Some(&packet), &mut out)
            .map_err(|e| format!("packet {packets}: {e}"))?;
        if dec.final_range() != enc.final_range() {
            return Err(format!(
                "packet {packets}: the decoder's final range differs"
            ));
        }
        packets += 1;
        bytes += packet.len();
    }
    let delay = enc.lookahead() * channels;
    let (mut sig, mut err) = (0.0f64, 0.0f64);
    for (w, g) in pcm.iter().zip(&out[delay..]) {
        sig += f64::from(*w).powi(2);
        err += f64::from(w - g).powi(2);
    }
    Ok((packets, bytes, 10.0 * (sig / err.max(1e-30)).log10()))
}

#[test]
#[ignore = "tier 3: needs the corpus (just corpus); run with just conformance"]
fn the_encoder_keeps_its_frozen_sizes_and_quality() {
    let mut text = String::from(FROZEN_HEADER);
    let mut failures = Vec::new();
    for n in VECTORS {
        let path = crossfade_corpus::require(&format!("opus/rfc8251/testvector{n}.dec"));
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let stereo: Vec<f32> = read_pcm16(&bytes)
            .iter()
            .take(48_000 * 2 * SECONDS)
            .map(|&s| f32::from(s) / 32768.0)
            .collect();
        let mono: Vec<f32> = stereo
            .chunks_exact(2)
            .map(|f| 0.5 * (f[0] + f[1]))
            .collect();
        for (name, is_stereo, bitrate, vbr) in SETTINGS {
            let (pcm, channels) = if is_stereo { (&stereo, 2) } else { (&mono, 1) };
            match run(pcm, channels, bitrate, vbr) {
                Ok((packets, bytes, snr)) => {
                    text.push_str(&format!(
                        "testvector{n} {name} {packets} {bytes} {snr:.1}\n"
                    ));
                    // Whatever is frozen, an encoder this far off its rate or this noisy is
                    // broken.
                    let seconds = packets as f64 * 0.02;
                    let rate = bytes as f64 * 8.0 / seconds;
                    let (lo, hi) = if vbr { (0.3, 1.9) } else { (0.99, 1.01) };
                    if !(lo * f64::from(bitrate)..hi * f64::from(bitrate)).contains(&rate) {
                        failures.push(format!("testvector{n} {name}: {rate:.0} b/s"));
                    }
                    if snr < 6.0 {
                        failures.push(format!("testvector{n} {name}: {snr:.1} dB"));
                    }
                }
                Err(why) => {
                    text.push_str(&format!("testvector{n} {name} error {why}\n"));
                    failures.push(format!("testvector{n} {name}: {why}"));
                }
            }
        }
    }
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
