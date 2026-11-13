#![no_main]

use crossfade_fuzz::{Pipe, limits, read_all, seek_and_read};
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

fuzz_target!(|data: &[u8]| {
    let Ok(mut demux) = crossfade_format::ogg::open(Box::new(Pipe::new(data)), &limits()) else {
        return;
    };
    let _ = read_all(&mut *demux, data.len());
    // Again from a source that can seek.
    if let Ok(mut demux) =
        crossfade_format::ogg::open(Box::new(Cursor::new(data.to_vec())), &limits())
    {
        seek_and_read(&mut *demux, data);
    }
});
