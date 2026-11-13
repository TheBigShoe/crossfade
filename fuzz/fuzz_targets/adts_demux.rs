#![no_main]
//! Reading ADTS never panics or hangs, on a pipe or a seekable source, and seeking (to places
//! taken from the input) lands where it says and reads on from there.

use crossfade_core::Packet;
use crossfade_fuzz::{Pipe, limits, read_all};
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

fuzz_target!(|data: &[u8]| {
    if let Ok(mut demux) = crossfade_format::adts::open(Box::new(Pipe::new(data)), &limits()) {
        let _ = read_all(&mut *demux, data.len());
    }
    let Ok(mut demux) =
        crossfade_format::adts::open(Box::new(Cursor::new(data.to_vec())), &limits())
    else {
        return;
    };
    let total = demux.streams()[0].frames;
    for chunk in data.chunks(8).take(4) {
        let mut b = [0u8; 8];
        b[..chunk.len()].copy_from_slice(chunk);
        let pts = i64::from_le_bytes(b) % (1 << 24);
        let Ok(landed) = demux.seek(pts) else {
            continue;
        };
        assert!(landed <= pts.max(0), "{landed} past {pts}");
        assert!(total.is_none_or(|t| landed as u64 <= t));
        let mut p = Packet::default();
        if let Ok(true) = demux.read_packet(&mut p) {
            assert_eq!(p.pts, landed);
        }
    }
    let _ = read_all(&mut *demux, data.len());
});
