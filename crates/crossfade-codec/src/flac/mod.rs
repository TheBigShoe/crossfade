//! FLAC (RFC 9639): the engine-facing adapters over `crossfade-flac` and the `flacenc` encoder.

pub mod decode;
pub mod encode;

// Kept so `crossfade-ui`, which another change owns, still builds; it should import
// `crossfade_flac` itself and this line then goes.
pub use crossfade_flac::streaminfo;
