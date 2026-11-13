#![no_main]
//! The Ogg Vorbis mapping never panics or hangs, from a pipe or a seekable source, reading,
//! seeking and decoding. Inputs build a valid Ogg Vorbis file (block sizes mixed, priming,
//! late start, padding, pages of any size, packets whose mode does not exist) and then
//! damage it, so that the fuzzer works on the mapping rather than on finding a valid setup.

use arbitrary::{Arbitrary, Unstructured};
use crossfade_fuzz::{Pipe, decode_all, limits, read_all, seek_and_read};
use crossfade_vorbis::testing::{self as w, OggLayout};
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let (Ok(channels), Ok(seq), Ok(seed), Ok(per_page), Ok(start), Ok(trim)) = (
        u.int_in_range(1u8..=3),
        Vec::<u8>::arbitrary(&mut u),
        u64::arbitrary(&mut u),
        u.int_in_range(1usize..=9),
        u.int_in_range(-3000i64..=3000),
        u.int_in_range(0i64..=600),
    ) else {
        return;
    };
    let seq: String = seq
        .iter()
        .take(48)
        .map(|b| ['S', 'L', 'S', 'L', 'X'][usize::from(b % 5)])
        .collect();
    let s = w::sample_stream(channels, &seq, seed);
    let layout = OggLayout {
        packets_per_page: vec![per_page],
        start,
        end_trim: trim,
        ..OggLayout::default()
    };
    let mut bytes = w::ogg_file([&s.headers[0], &s.headers[1], &s.headers[2]], &s.audio, &layout);
    // Damage: (offset, xor) pairs and a cut, from the rest of the input.
    while let (Ok(at), Ok(x)) = (u32::arbitrary(&mut u), u8::arbitrary(&mut u)) {
        if u.is_empty() && x == 0 {
            break;
        }
        let len = bytes.len();
        bytes[at as usize % len] ^= x;
    }
    if let Ok(cut) = u32::arbitrary(&mut u) {
        if cut % 4 == 0 {
            let len = bytes.len();
            bytes.truncate(cut as usize % (len + 1));
        }
    }
    if let Ok(mut d) = crossfade_format::ogg::open(Box::new(Pipe::new(&bytes)), &limits()) {
        let _ = read_all(&mut *d, bytes.len());
    }
    if let Ok(mut d) = crossfade_format::ogg::open(Box::new(Cursor::new(bytes.clone())), &limits()) {
        seek_and_read(&mut *d, &bytes);
    }
    if let Ok(mut d) = crossfade_format::ogg::open(Box::new(Cursor::new(bytes.clone())), &limits()) {
        let _ = decode_all(&mut *d, bytes.len());
    }
});
