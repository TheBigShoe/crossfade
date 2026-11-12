#![no_main]
//! The MP3 reader, tags included, on a pipe and on a seekable input with a few seeks, decoding
//! what it reads: never a panic or a hang.

use crossfade_codec::decoder_for;
use crossfade_core::{AudioFrame, Demuxer, Packet};
use crossfade_fuzz::{Pipe, limits, read_all};
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

fn decode_some(d: &mut dyn Demuxer, budget: usize) {
    let stream = d.streams()[0].clone();
    let Ok(mut dec) = decoder_for(&stream) else {
        return;
    };
    let (mut p, mut f) = (Packet::default(), AudioFrame::new(&stream.audio));
    for _ in 0..budget {
        match d.read_packet(&mut p) {
            Ok(true) => {}
            _ => return,
        }
        if dec.send_packet(&p).is_err() {
            dec.reset();
            continue;
        }
        while let Ok(true) = dec.receive_frame(&mut f) {}
    }
}

fuzz_target!(|data: &[u8]| {
    if let Ok(mut d) = crossfade_format::mp3::open(Box::new(Pipe::new(data)), None, &limits()) {
        let _ = read_all(&mut *d, data.len());
    }
    // Through sniffing (ID3v2 tags in front), seekable, so the trailing tags and seeks run.
    let Ok((_, mut d)) =
        crossfade_format::open_demuxer(Box::new(Cursor::new(data.to_vec())), &limits())
    else {
        return;
    };
    let _ = d.tags();
    decode_some(&mut *d, data.len() + 16);
    let seeds = data.iter().rev().take(4);
    for (i, &b) in seeds.enumerate() {
        let target = i64::from(b) * 97 - 600 + i as i64 * 1152;
        if d.seek(target).is_ok() {
            decode_some(&mut *d, 8);
        }
    }
});
