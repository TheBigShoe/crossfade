#![no_main]
//! The AAC decoder (built with its `decoder` feature) never panics or hangs on any
//! configuration and blocks, and decodes the same after a reset. Input: a length byte and that
//! many bytes of AudioSpecificConfig, then blocks, each behind a length byte.

use crossfade_aac::Decoder;
use crossfade_aac::asc::AudioSpecificConfig;
use libfuzzer_sys::fuzz_target;

fn blocks(mut rest: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    while let Some((&n, tail)) = rest.split_first() {
        let n = usize::from(n).min(tail.len());
        out.push(&tail[..n]);
        rest = &tail[n..];
    }
    out
}

fn run(d: &mut Decoder, blocks: &[&[u8]]) -> Vec<Result<Vec<u32>, crossfade_aac::Error>> {
    let mut out = Vec::new();
    blocks
        .iter()
        .enumerate()
        .map(|(i, b)| {
            d.decode_blocks(b, 1 + i % 2, &mut out)
                .map(|_| out.iter().map(|s| s.to_bits()).collect())
        })
        .collect()
}

fuzz_target!(|data: &[u8]| {
    let Some((&n, rest)) = data.split_first() else {
        return;
    };
    let n = usize::from(n % 16).min(rest.len());
    let Ok(config) = AudioSpecificConfig::parse(&rest[..n]) else {
        return;
    };
    let Ok(mut d) = Decoder::new(&config) else {
        return;
    };
    let blocks = blocks(&rest[n..]);
    let first = run(&mut d, &blocks);
    d.reset();
    assert!(first == run(&mut d, &blocks), "a reset decoder decodes differently");
});
