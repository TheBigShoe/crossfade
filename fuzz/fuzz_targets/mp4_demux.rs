#![no_main]
//! The MP4 reader never panics or hangs on a pipe or a seekable input, seeking included, and
//! what it reads decodes (ALAC) without panicking.

use crossfade_codec::decoder_for;
use crossfade_core::{AudioFrame, Demuxer, Packet};
use crossfade_fuzz::{Pipe, limits};
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

/// Reads (and decodes, when a decoder exists) to the end or the first error. Samples of 4-bit
/// `stz2` sizes may be empty, two to a byte, so the bound is twice the input.
fn drain(demux: &mut dyn Demuxer, data_len: usize) {
    let stream = demux.streams()[0].clone();
    let mut decoder = decoder_for(&stream).ok();
    let (mut packet, mut frame) = (Packet::default(), AudioFrame::new(&stream.audio));
    for _ in 0..2 * data_len + 16 {
        match demux.read_packet(&mut packet) {
            Ok(true) => {}
            _ => return,
        }
        if let Some(d) = decoder.as_mut() {
            if d.send_packet(&packet).is_err() {
                decoder = None;
                continue;
            }
            while let Ok(true) = d.receive_frame(&mut frame) {}
        }
    }
    panic!("read loop did not end");
}

fuzz_target!(|data: &[u8]| {
    if let Ok(mut demux) = crossfade_format::mp4::open(Box::new(Pipe::new(data)), &limits()) {
        drain(&mut *demux, data.len());
    }
    let seekable = Box::new(Cursor::new(data.to_vec()));
    if let Ok(mut demux) = crossfade_format::mp4::open(seekable, &limits()) {
        let frames = demux.streams()[0].frames.unwrap_or(0) as i64;
        for target in [frames / 2, -1, frames, 0] {
            // At most the target, or the start (at or before 0) when the target is before it.
            if let Ok(at) = demux.seek(target) {
                assert!(at <= target.max(0), "{at} > {target}");
            }
            let mut packet = Packet::default();
            let _ = demux.read_packet(&mut packet);
        }
        drain(&mut *demux, data.len());
    }
});
