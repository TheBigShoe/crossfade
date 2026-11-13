#![no_main]
//! CELT frames through the Opus decoder: the input is a sequence of packets for one decoder,
//! each a CELT TOC the input chooses (configs 16-31, mono or stereo, codes 0-3) over frame
//! bytes the input gives, so every byte the fuzzer finds reaches the CELT layer. Never a
//! panic or a hang; a packet decodes to exactly its duration of finite samples, or is an error
//! that appends nothing; and a reset decoder decodes the same packets to the same bits.

use arbitrary::{Result, Unstructured};
use crossfade_opus::{Decoder, packet_duration};
use libfuzzer_sys::fuzz_target;

/// A packet: a CELT TOC from two input bytes, then up to 1500 bytes taken as they are (the
/// framing after the TOC, so codes 1-3 parse their lengths from the input).
fn packet(u: &mut Unstructured<'_>) -> Result<Vec<u8>> {
    let config = 16 + u.int_in_range(0..=15u8)?;
    let stereo = u8::from(u.arbitrary::<bool>()?);
    let code = u.int_in_range(0..=3u8)?;
    let len = u.int_in_range(0..=1500usize)?.min(u.len());
    let mut p = vec![(config << 3) | (stereo << 2) | code];
    p.extend_from_slice(u.bytes(len)?);
    Ok(p)
}

fn run(data: &[u8]) -> Result<()> {
    let mut u = Unstructured::new(data);
    let channels = if u.arbitrary::<bool>()? { 2 } else { 1 };
    let mut packets = vec![];
    while !u.is_empty() && packets.len() < 8 {
        packets.push(packet(&mut u)?);
    }
    let mut d = Decoder::new(channels).expect("1 or 2 channels");
    let mut first = vec![];
    for p in &packets {
        let at = first.len();
        match d.decode(Some(p), &mut first) {
            Ok(n) => {
                assert_eq!(Ok(n), packet_duration(p));
                assert_eq!(first.len(), at + n * channels);
                assert!(first[at..].iter().all(|v| v.is_finite()));
            }
            Err(_) => assert_eq!(first.len(), at),
        }
    }
    d.reset();
    let mut again = vec![];
    for p in &packets {
        let _ = d.decode(Some(p), &mut again);
    }
    let same = first.len() == again.len()
        && first.iter().zip(&again).all(|(a, b)| a.to_bits() == b.to_bits());
    assert!(same, "a reset decoder decodes differently");
    Ok(())
}

fuzz_target!(|data: &[u8]| {
    let _ = run(data);
});
