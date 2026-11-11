#![no_main]
//! Decoding never panics or hangs, and on two or three threads it shows exactly what it shows
//! on one: every frame, and the same error.

use crossfade_fuzz::{Pipe, decode_all, decode_outcome, limits};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let open = || crossfade_format::flac::open(Box::new(Pipe::new(data)), &limits());
    let Ok(mut demux) = open() else {
        return;
    };
    let _ = decode_all(&mut *demux, data.len());
    let threads = 2 + data.len() % 2;
    let (Ok(mut one), Ok(mut many)) = (open(), open()) else {
        panic!("the reader opened once and not again");
    };
    let want = decode_outcome(&mut *one, data.len(), 1);
    let got = decode_outcome(&mut *many, data.len(), threads);
    assert!(
        (got.1, got.2) == (want.1, want.2),
        "{threads} threads differ from one"
    );
});
