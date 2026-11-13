#![no_main]
//! Opus parsing and the range decoder on any bytes: the packet parser (both framings) and
//! `packet_duration`, the Ogg header parsers, the range decoder's primitives driven by the
//! input, and the top-level decoder. Never a panic or a hang; parsed frames lie inside the
//! input, and the range decoder's `tell` only grows and equals `ceil(tell_frac / 8)`.

use crossfade_opus::packet::{MAX_FRAME_LEN, MAX_FRAMES, MAX_PACKET_SAMPLES};
use crossfade_opus::{Decoder, OpusHead, OpusTags, Packet, RangeDecoder, packet_duration};
use libfuzzer_sys::fuzz_target;

fn packets(data: &[u8]) {
    for p in [Packet::parse(data), Packet::parse_self_delimited(data)]
        .into_iter()
        .flatten()
    {
        assert!((1..=MAX_FRAMES).contains(&p.frames().len()));
        assert!(p.duration() <= MAX_PACKET_SAMPLES);
        assert!(p.byte_len() <= data.len());
        let inside: usize = p.frames().iter().map(|f| f.len()).sum::<usize>() + p.padding().len();
        assert!(inside < p.byte_len());
        assert!(p.frames().iter().all(|f| f.len() <= MAX_FRAME_LEN));
        assert_eq!(packet_duration(data), Ok(p.duration()));
    }
    let _ = packet_duration(data);
}

fn headers(data: &[u8]) {
    let _ = OpusHead::parse(data);
    let _ = OpusTags::parse(data);
    // Past the magic, so the fields are reached.
    let mut head = b"OpusHead".to_vec();
    head.extend_from_slice(data);
    if let Ok(h) = OpusHead::parse(&head) {
        assert_eq!(h.mapping.table.len(), usize::from(h.channels));
    }
    let mut tags = b"OpusTags".to_vec();
    tags.extend_from_slice(data);
    if let Ok(t) = OpusTags::parse(&tags) {
        assert_eq!(t.body.len() + t.extra.len() + 8, tags.len());
    }
}

/// The first half of the input is the frame, the second a script of operations: three bytes
/// each, the first choosing the primitive, the others its argument. Valid arguments keep the
/// invariants checked; a byte with its top bit set passes arbitrary ones, which must not
/// panic either.
fn range(data: &[u8]) {
    let (frame, script) = data.split_at(data.len() / 2);
    let mut dec = RangeDecoder::new(frame);
    let mut valid = true;
    let icdf8: [u8; 4] = [200, 120, 40, 0];
    let icdf16: [u16; 4] = [30000, 9000, 100, 0];
    for op in script.as_chunks::<3>().0 {
        let arg = u32::from(op[1]) << 8 | u32::from(op[2]);
        let before = (dec.tell(), dec.tell_frac());
        if op[0] & 0x80 != 0 {
            valid = false;
            let wild = arg.wrapping_mul(0x9E37_79B9);
            match op[0] % 9 {
                0 => {
                    let _ = dec.decode(wild);
                }
                1 => {
                    let _ = dec.decode_bin(wild);
                }
                2 => dec.update(wild >> 16, wild & 0xFFFF, arg),
                3 => {
                    let _ = dec.dec_bit_logp(wild);
                }
                4 => {
                    let _ = dec.dec_icdf(&icdf8, wild);
                }
                5 => {
                    let _ = dec.dec_icdf16(&icdf16[..usize::from(op[1] % 5)], wild);
                }
                6 => {
                    let _ = dec.dec_uint(wild);
                }
                7 => {
                    let _ = dec.dec_bits(wild);
                }
                _ => dec.shrink(wild as usize),
            }
            continue;
        }
        match op[0] % 7 {
            0 => {
                let ft = arg.max(1);
                let fs = dec.decode(ft);
                assert!(fs < ft);
                dec.update(fs, fs + 1, ft);
            }
            1 => {
                let bits = arg % 16;
                let fs = dec.decode_bin(bits);
                assert!(fs < 1 << bits);
                dec.update(fs, fs + 1, 1 << bits);
            }
            2 => {
                let _ = dec.dec_bit_logp(1 + arg % 15);
            }
            3 => assert!(dec.dec_icdf(&icdf8, 8) < 4),
            4 => assert!(dec.dec_icdf16(&icdf16, 15) < 4),
            5 => {
                let ft = (arg << 16 | arg).max(2);
                assert!(dec.dec_uint(ft) < ft);
            }
            _ => {
                let bits = arg % 26;
                assert!(dec.dec_bits(bits) < 1 << bits);
            }
        }
        if valid {
            assert!(dec.tell() >= before.0);
            assert!(dec.tell_frac() >= before.1);
            assert_eq!(dec.tell() as u32, dec.tell_frac().div_ceil(8));
        }
    }
    let _ = (dec.rng(), dec.error(), dec.storage(), dec.range_bytes());
}

fn decoder(data: &[u8]) {
    for channels in [1, 2] {
        let mut d = Decoder::new(channels).unwrap();
        let mut out = vec![];
        match d.decode(Some(data), &mut out) {
            Ok(n) => assert_eq!(out.len(), n * channels),
            Err(_) => assert!(out.is_empty()),
        }
        let _ = d.decode(None, &mut out);
    }
}

fuzz_target!(|data: &[u8]| {
    packets(data);
    headers(data);
    range(data);
    decoder(data);
});
