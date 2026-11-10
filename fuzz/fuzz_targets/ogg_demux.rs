#![no_main]

use crossfade_fuzz::{Pipe, limits, read_all};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(mut demux) = crossfade_format::ogg::open(Box::new(Pipe::new(data)), &limits()) else {
        return;
    };
    let _ = read_all(&mut *demux, data.len());
});
