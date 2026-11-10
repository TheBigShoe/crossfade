#![no_main]

use crossfade_fuzz::{Pipe, decode_all, limits};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(mut demux) = crossfade_format::flac::open(Box::new(Pipe::new(data)), &limits()) else {
        return;
    };
    let _ = decode_all(&mut *demux, data.len());
});
