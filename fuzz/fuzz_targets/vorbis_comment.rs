#![no_main]

use crossfade_fuzz::limits;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = crossfade_format::vorbis_comment::parse(data, &limits());
});
