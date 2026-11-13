//! Decoding a test vector the way `opus_demo -d 48000 2` does, checking the decoder's final
//! range against the encoder's after every packet: the conformance tier's engine, shared by
//! `crossfade-format/tests/conformance_opus.rs` and the `opus_vector` example.

use crate::testing::bitfile::read_bit_file;
use crate::{Bandwidth, Decoder, Mode, Toc, packet_duration};
use std::collections::BTreeSet;

/// libopus's `FLOAT2INT16` (float build): scale, saturate, round half to even (`lrintf`).
/// NaN gives -32768, as the C's comparisons do.
pub fn float_to_pcm16(x: f32) -> i16 {
    let x = x * 32768.0;
    let x = if x > -32768.0 { x } else { -32768.0 };
    let x = if x < 32767.0 { x } else { 32767.0 };
    x.round_ties_even() as i16
}

/// A TOC for people: `config 31 CELT FB 20 ms stereo code 3`.
pub fn describe(toc: Toc) -> String {
    let mode = match toc.mode() {
        Mode::Silk => "SILK",
        Mode::Hybrid => "Hybrid",
        Mode::Celt => "CELT",
    };
    let bw = match toc.bandwidth() {
        Bandwidth::Narrow => "NB",
        Bandwidth::Medium => "MB",
        Bandwidth::Wide => "WB",
        Bandwidth::SuperWide => "SWB",
        Bandwidth::Full => "FB",
    };
    let ms = toc.frame_samples() as f64 / 48.0;
    let ch = if toc.stereo() { "stereo" } else { "mono" };
    format!(
        "config {} {mode} {bw} {ms} ms {ch} code {}",
        toc.config(),
        toc.code()
    )
}

/// The modes a `.bit` file's packets use (lost packets have none).
pub fn modes(bit: &[u8]) -> Result<BTreeSet<&'static str>, String> {
    Ok(read_bit_file(bit)?
        .iter()
        .filter_map(|p| p.data.first())
        .map(|&t| match Toc(t).mode() {
            Mode::Silk => "silk",
            Mode::Hybrid => "hybrid",
            Mode::Celt => "celt",
        })
        .collect())
}

/// Decodes a `.bit` file with a [`Decoder`] of `channels` channels, as `opus_demo` does: a
/// packet of length 0 is lost (decoded as `None`), and after every packet whose stored final
/// range is not 0, unless it or the one before was lost, the decoder's final range must equal
/// it. Returns the output as `opus_demo` writes it (16-bit, [`float_to_pcm16`]), or the first
/// failure, naming the packet's index and TOC. `log` gets one line per packet.
pub fn decode_vector(
    bit: &[u8],
    channels: usize,
    mut log: impl FnMut(&str),
) -> Result<Vec<i16>, String> {
    let packets = read_bit_file(bit)?;
    let mut decoder = Decoder::new(channels).map_err(|e| e.to_string())?;
    let mut pcm = Vec::new();
    let mut prev_lost = false;
    for (i, p) in packets.iter().enumerate() {
        let lost = p.data.is_empty();
        let what = p
            .data
            .first()
            .map_or("lost".to_string(), |&t| describe(Toc(t)));
        let at = pcm.len();
        decoder
            .decode((!lost).then_some(p.data), &mut pcm)
            .map_err(|e| format!("packet {i} ({what}): {e}"))?;
        let check = p.final_range != 0 && !lost && !prev_lost;
        let ours = decoder.final_range();
        let samples = (pcm.len() - at) / channels;
        let expected = p.data.first().map(|_| packet_duration(p.data));
        log(&format!(
            "{i:5} {what:40} {samples:5} samples  range {ours:#010x} {}",
            match (check, ours == p.final_range) {
                (false, _) => "(not checked)".to_string(),
                (true, true) => "ok".to_string(),
                (true, false) => format!("MISMATCH, expected {:#010x}", p.final_range),
            }
        ));
        if check && ours != p.final_range {
            return Err(format!(
                "packet {i} ({what}): final range {ours:#010x}, expected {:#010x}",
                p.final_range
            ));
        }
        if let Some(Ok(d)) = expected
            && d != samples
        {
            return Err(format!(
                "packet {i} ({what}): {samples} samples decoded, the TOC says {d}"
            ));
        }
        prev_lost = lost;
    }
    Ok(pcm.iter().map(|&s| float_to_pcm16(s)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bit(packets: &[(&[u8], u32)]) -> Vec<u8> {
        let mut v = vec![];
        for (data, rng) in packets {
            v.extend((data.len() as u32).to_be_bytes());
            v.extend(rng.to_be_bytes());
            v.extend_from_slice(data);
        }
        v
    }

    #[test]
    fn float_to_pcm16_is_float2int16() {
        assert_eq!(float_to_pcm16(0.0), 0);
        assert_eq!(float_to_pcm16(1.0), 32767);
        assert_eq!(float_to_pcm16(-1.0), -32768);
        assert_eq!(float_to_pcm16(-2.0), -32768);
        assert_eq!(float_to_pcm16(f32::NAN), -32768);
        // Half a step rounds to even.
        assert_eq!(float_to_pcm16(0.5 / 32768.0), 0);
        assert_eq!(float_to_pcm16(1.5 / 32768.0), 2);
        assert_eq!(float_to_pcm16(-2.5 / 32768.0), -2);
    }

    #[test]
    fn describes_a_toc() {
        assert_eq!(
            describe(Toc((31 << 3) | 0b111)),
            "config 31 CELT FB 20 ms stereo code 3"
        );
        assert_eq!(
            describe(Toc(16 << 3)),
            "config 16 CELT NB 2.5 ms mono code 0"
        );
    }

    #[test]
    fn decodes_and_checks_every_packet() {
        // Two DTX packets (a TOC alone): silence, final range 0, and a lost one before any
        // audio, which yields nothing.
        let file = bit(&[(&[31 << 3], 0), (&[], 0), (&[(1 << 3) | 0b100], 0)]);
        let mut lines = vec![];
        let pcm = decode_vector(&file, 2, |l| lines.push(l.to_string())).unwrap();
        assert_eq!(pcm, vec![0; 2 * 960 * 2]);
        assert_eq!(lines.len(), 3);
        assert!(
            lines[0].contains("config 31 CELT FB 20 ms mono code 0"),
            "{}",
            lines[0]
        );
        assert_eq!(modes(&file).unwrap(), BTreeSet::from(["celt", "silk"]));
    }

    #[test]
    fn reports_the_first_mismatch_with_its_packet_and_toc() {
        let file = bit(&[(&[31 << 3], 0), (&[31 << 3], 0x1234)]);
        assert_eq!(
            decode_vector(&file, 2, |_| {}).unwrap_err(),
            "packet 1 (config 31 CELT FB 20 ms mono code 0): final range 0x00000000, expected 0x00001234"
        );
        // A packet after a lost one is not checked.
        let file = bit(&[(&[31 << 3], 0), (&[], 0), (&[31 << 3], 0x1234)]);
        assert!(decode_vector(&file, 2, |_| {}).is_ok());
    }

    #[test]
    fn reports_a_layer_that_is_not_built() {
        // CELT and SILK are built; hybrid is the joining stage's.
        let file = bit(&[(&[(15 << 3) | 0b100, 1, 2, 3], 0x1234)]);
        assert_eq!(
            decode_vector(&file, 2, |_| {}).unwrap_err(),
            "packet 0 (config 15 Hybrid FB 20 ms stereo code 0): opus: hybrid decoding is not built yet"
        );
    }
}
