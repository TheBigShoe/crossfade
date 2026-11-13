#![no_main]
//! The Ogg Opus mapping, and the engine's Opus adapter behind it, never panic or hang, from a
//! pipe or a seekable source, reading, seeking and decoding. Inputs build a valid Ogg Opus
//! file (every mode and frame size, mono to multistream, pre-skip, a cut front, end trimming,
//! pages of any size) and then damage it, so that the fuzzer works on the mapping rather than
//! on finding valid headers.

use arbitrary::{Arbitrary, Unstructured};
use crossfade_fuzz::{Pipe, decode_all, limits, read_all, seek_and_read};
use crossfade_opus::testing::ogg::{self as w, OggLayout};
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let (Ok(multi), Ok(configs), Ok(seed), Ok(per_page), Ok(start), Ok(trim), Ok(pre_skip)) = (
        bool::arbitrary(&mut u),
        Vec::<u8>::arbitrary(&mut u),
        u64::arbitrary(&mut u),
        u.int_in_range(1usize..=9),
        u.int_in_range(-200i64..=100_000),
        u.int_in_range(0i64..=2000),
        u.int_in_range(0u16..=8000),
    ) else {
        return;
    };
    let mut audio = vec![];
    for (i, c) in configs.iter().take(40).enumerate() {
        let config = c & 31;
        let stereo = c & 0x20 != 0;
        let mut p = w::packets(config, stereo, 1, usize::from(c >> 6) * 30 + 1, seed.wrapping_add(i as u64));
        if multi {
            let mono = w::packets(config, false, 1, 20, seed.wrapping_add(1000 + i as u64));
            p = vec![w::multistream_packet(&[p[0].clone(), mono[0].clone()])];
        }
        audio.extend(p);
    }
    let head = if multi {
        w::opus_head(3, pre_skip, 0, Some((1, 2, 1, &[0, 2, 1])))
    } else {
        w::opus_head(2, pre_skip, -256, None)
    };
    let layout = OggLayout {
        packets_per_page: vec![per_page],
        start,
        end_trim: trim,
        ..OggLayout::default()
    };
    let mut bytes = w::ogg_file(&head, &w::opus_tags("fuzz", &["TITLE=x"]), &audio, &layout);
    // Damage, when asked: (offset, xor) pairs and a cut, from the rest of the input.
    let damage = u8::arbitrary(&mut u).unwrap_or(0) % 4 != 0;
    while damage
        && let (Ok(at), Ok(x)) = (u32::arbitrary(&mut u), u8::arbitrary(&mut u)) {
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
