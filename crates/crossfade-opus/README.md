# crossfade-opus

Opus decoding in pure Rust, depending only on `libm`: the range decoder (RFC 6716 section 4.1),
the packet parser (section 3 and the self-delimiting framing of Appendix B), the Ogg
identification and comment headers as data (RFC 7845 section 5), and the decoder's frame,
into which the CELT and SILK layers are being built. RFC 8251's corrections apply throughout.

This crate is a port of the reference decoder of RFC 6716 and of libopus
(<https://github.com/xiph/opus>), and is therefore licensed, unlike the rest of the Crossfade
workspace, under the 3-clause BSD licence of those sources: see `COPYING`, which also points to
the royalty-free patent licences that cover Opus. `src/testing/compare.rs` is a port of
`opus_compare.c` and keeps its own 2-clause BSD notice.
