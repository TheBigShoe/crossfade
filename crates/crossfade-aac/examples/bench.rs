//! Decodes an ADTS stream in memory, `repeat` times, and prints the time per pass and a hash
//! of the output bits: the in-process benchmark for decoder speed work, where a change must
//! keep the hash. Measure instructions with `/usr/bin/time -l`:
//! `cargo build --release -p crossfade-aac --features decoder --example bench`, then
//! `/usr/bin/time -l target/release/examples/bench in.aac [repeat]`.

use crossfade_aac::Decoder;
use crossfade_aac::adts::Header;
use crossfade_aac::asc::AudioSpecificConfig;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let (input, repeat) = match &args[..] {
        [_, input] => (input, 1),
        [_, input, n] => (input, n.parse()?),
        _ => return Err("usage: bench <in.aac> [repeat]".into()),
    };
    let bytes = std::fs::read(input)?;
    let first = Header::parse(&bytes)?;
    let config = AudioSpecificConfig::parse(&first.audio_specific_config())?;
    let mut blocks: Vec<(&[u8], usize)> = Vec::new();
    let mut at = 0;
    while at + Header::MIN_LEN <= bytes.len() {
        let h = Header::parse(&bytes[at..])?;
        let end = at + usize::from(h.frame_length);
        if end > bytes.len() {
            break;
        }
        blocks.extend(
            h.raw_data_blocks(&bytes[at..end])?
                .map(|b| (b.data, usize::from(b.blocks))),
        );
        at = end;
    }
    let mut decoder = Decoder::new(&config)?;
    let mut pcm = Vec::new();
    for _ in 0..repeat {
        decoder.reset();
        // FNV-1a over the output bits, 32 bits at a time.
        let mut hash = 0xCBF2_9CE4_8422_2325u64;
        let start = Instant::now();
        for &(block, n) in &blocks {
            decoder.decode_blocks(block, n, &mut pcm)?;
            for s in &pcm {
                hash = (hash ^ u64::from(s.to_bits())).wrapping_mul(0x100_0000_01B3);
            }
        }
        println!(
            "{} blocks in {:.1} ms, hash {hash:016x}",
            blocks.len(),
            start.elapsed().as_secs_f64() * 1e3
        );
    }
    Ok(())
}
