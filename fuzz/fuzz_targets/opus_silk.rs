#![no_main]
//! SILK on any bytes, through the public decoder: the input is a stream of packets, each a
//! control byte (the SILK config 0-11 in the low nibble, the framing code in bits 4-5, the
//! stereo flag in bit 6; 0xFF alone is a lost packet), a length and that many payload bytes,
//! sent after a TOC built from the control byte. A mono and a stereo decoder each take the
//! whole stream, so state carries over rate, frame-size and mono/stereo switches. Never a
//! panic or hang; every packet decodes to its length or fails with nothing appended.

use crossfade_opus::{Decoder, Packet};
use libfuzzer_sys::fuzz_target;

fn packets(mut data: &[u8]) -> Vec<Option<Vec<u8>>> {
    let mut out = vec![];
    while let [ctl, len, rest @ ..] = data {
        if *ctl == 0xFF {
            out.push(None);
            data = &data[1..];
            continue;
        }
        let len = usize::from(*len).min(rest.len());
        let (payload, tail) = rest.split_at(len);
        let config = (ctl & 0x0F) % 12;
        let stereo = ctl & 0x40 != 0;
        let code = (ctl >> 4) & 3;
        let mut p = vec![(config << 3) | (u8::from(stereo) << 2) | code];
        p.extend_from_slice(payload);
        out.push(Some(p));
        data = tail;
    }
    out
}

fuzz_target!(|data: &[u8]| {
    let stream = packets(data);
    for channels in [1, 2] {
        let mut d = Decoder::new(channels).unwrap();
        for p in &stream {
            let mut out = vec![];
            match d.decode(p.as_deref(), &mut out) {
                Ok(n) => {
                    assert_eq!(out.len(), n * channels);
                    if let Some(p) = p {
                        assert_eq!(n, Packet::parse(p).unwrap().duration());
                    }
                    // Not bounded by full scale: spare bytes carry a redundant CELT frame.
                    assert!(out.iter().all(|s| s.is_finite()));
                }
                Err(_) => assert!(out.is_empty()),
            }
        }
    }
});
