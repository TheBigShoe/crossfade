#![no_main]
//! The whole Opus decoder, single-stream and multistream, on packet streams of any mode with
//! losses, concealment, FEC calls and resets: never a panic or hang; every call appends its
//! length or fails with nothing appended. The input is a script: per step a control byte
//! (low 3 bits: decode, lose, FEC, conceal, reset; the rest a length or a 2.5 ms count) and
//! the packet's bytes (a TOC from the input, so every mode and framing is met).

use arbitrary::{Arbitrary, Unstructured};
use crossfade_opus::{ChannelMapping, Decoder, MultistreamDecoder, Packet};
use libfuzzer_sys::fuzz_target;

#[derive(Debug)]
enum Step {
    Decode(Vec<u8>),
    Lose,
    Fec(Vec<u8>),
    Conceal(usize),
    Reset,
}

fn script(u: &mut Unstructured<'_>) -> Vec<Step> {
    let mut steps = vec![];
    while let Ok(ctl) = u8::arbitrary(u) {
        let len = usize::from(ctl >> 3) * 12;
        let bytes = |u: &mut Unstructured<'_>| u.bytes(len.min(u.len())).unwrap_or(&[]).to_vec();
        steps.push(match ctl & 7 {
            0..=2 => Step::Decode(bytes(u)),
            3 => Step::Lose,
            4 => Step::Fec(bytes(u)),
            5 => Step::Conceal(usize::from(ctl >> 3) * 120),
            _ => Step::Reset,
        });
        if steps.len() >= 64 {
            break;
        }
    }
    steps
}

fn check(out: &[f32], r: Result<usize, crossfade_opus::Error>, channels: usize) {
    match r {
        Ok(n) => {
            assert_eq!(out.len(), n * channels);
            assert!(n <= 5760);
            assert!(out.iter().all(|s| s.is_finite()));
        }
        Err(_) => assert!(out.is_empty()),
    }
}

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let Ok(streams) = u.int_in_range(1u8..=3) else {
        return;
    };
    let Ok(coupled) = u.int_in_range(0..=streams) else {
        return;
    };
    let steps = script(&mut u);
    for channels in [1, 2] {
        let mut d = Decoder::new(channels).unwrap();
        for s in &steps {
            let mut out = vec![];
            let r = match s {
                Step::Decode(p) => {
                    let r = d.decode(Some(p), &mut out);
                    if let Ok(n) = r {
                        assert_eq!(n, Packet::parse(p).unwrap().duration());
                    }
                    r
                }
                Step::Lose => d.decode(None, &mut out),
                Step::Fec(p) => d.decode_fec(p, &mut out),
                Step::Conceal(n) => d.conceal(*n, &mut out),
                Step::Reset => {
                    d.reset();
                    continue;
                }
            };
            check(&out, r, channels);
        }
    }
    // Every decoded channel once, then one silent.
    let decoded = streams + coupled;
    let mut table: Vec<u8> = (0..decoded).collect();
    table.push(255);
    let mapping = ChannelMapping {
        family: 1,
        streams,
        coupled,
        table,
    };
    let channels = mapping.table.len();
    let mut ms = MultistreamDecoder::new(&mapping, channels).unwrap();
    for s in &steps {
        let mut out = vec![];
        let r = match s {
            Step::Decode(p) => ms.decode(Some(p), &mut out),
            Step::Lose => ms.decode(None, &mut out),
            Step::Fec(p) => ms.decode_fec(p, &mut out),
            Step::Conceal(n) => ms.conceal(*n, &mut out),
            Step::Reset => {
                ms.reset();
                continue;
            }
        };
        check(&out, r, channels);
    }
});
