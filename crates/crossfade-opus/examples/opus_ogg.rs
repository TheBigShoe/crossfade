//! Wraps an RFC 8251 test vector's packets into an Ogg Opus file (stereo head, pre-skip 312,
//! a few packets a page), for timing `crossfade convert` on real Opus audio. Nothing from the
//! vectors is committed: write the file to a scratch directory.
//!
//! ```sh
//! D=.corpus-cache/opus/rfc8251
//! cargo run --release -p crossfade-opus --example opus_ogg -- \
//!     $D/testvector06.bit/testvector06.bit /tmp/tv06.opus
//! ```

use crossfade_opus::testing::bitfile::read_bit_file;
use crossfade_opus::testing::ogg::{OggLayout, ogg_file, opus_head, opus_tags};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, output] = &args[..] else {
        panic!("usage: opus_ogg <in.bit> <out.opus>");
    };
    let bytes = std::fs::read(input).unwrap_or_else(|e| panic!("{input}: {e}"));
    let packets: Vec<Vec<u8>> = read_bit_file(&bytes)
        .unwrap_or_else(|e| panic!("{input}: {e}"))
        .iter()
        .map(|p| p.data.to_vec())
        .collect();
    let file = ogg_file(
        &opus_head(2, 312, 0, None),
        &opus_tags("crossfade opus_ogg", &[]),
        &packets,
        &OggLayout {
            packets_per_page: vec![10],
            ..OggLayout::default()
        },
    );
    std::fs::write(output, file).unwrap_or_else(|e| panic!("{output}: {e}"));
}
