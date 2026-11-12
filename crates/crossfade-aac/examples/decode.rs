//! Decodes an ADTS stream (`.aac`, `.adts`) to raw interleaved little-endian `f32`, for
//! measuring against reference decoders: `cargo run --release -p crossfade-aac --features
//! decoder --example decode -- in.aac out.f32`. A `.blk` input is an AudioSpecificConfig and
//! raw data blocks, each behind a little-endian `u32` length (the ASC first), as a scratch
//! extractor writes them from an MP4.

use crossfade_aac::Decoder;
use crossfade_aac::adts::Header;
use crossfade_aac::asc::AudioSpecificConfig;
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let [_, input, output] = &args[..] else {
        return Err("usage: decode <in.aac|in.blk> <out.f32>".into());
    };
    let bytes = std::fs::read(input)?;
    let mut blocks: Vec<&[u8]> = Vec::new();
    let config;
    if input.ends_with(".blk") {
        let mut at = 0;
        let mut items = Vec::new();
        while at + 4 <= bytes.len() {
            let n = u32::from_le_bytes(bytes[at..at + 4].try_into()?) as usize;
            items.push(&bytes[at + 4..at + 4 + n]);
            at += 4 + n;
        }
        config = AudioSpecificConfig::parse(items[0])?;
        blocks.extend(&items[1..]);
    } else {
        let first = Header::parse(&bytes)?;
        config = AudioSpecificConfig::parse(&first.audio_specific_config())?;
        let mut at = 0;
        while at + Header::MIN_LEN <= bytes.len() {
            let h = Header::parse(&bytes[at..])?;
            let end = at + usize::from(h.frame_length);
            if end > bytes.len() {
                break;
            }
            blocks.extend(h.raw_data_blocks(&bytes[at..end])?);
            at = end;
        }
    }
    let mut decoder = Decoder::new(&config)?;
    let mut out = std::io::BufWriter::new(std::fs::File::create(output)?);
    let mut pcm = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        decoder
            .decode(block, &mut pcm)
            .map_err(|e| format!("block {i}: {e}"))?;
        for s in &pcm {
            out.write_all(&s.to_le_bytes())?;
        }
    }
    out.flush()?;
    eprintln!(
        "{} blocks, {} channels, {} Hz, sbr {}",
        blocks.len(),
        decoder.channels(),
        decoder.sample_rate(),
        decoder.sbr_present()
    );
    Ok(())
}
