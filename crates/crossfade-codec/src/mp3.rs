//! MP3 decoder adapter. Not built yet.

#[cfg(test)]
use crossfade_core::{AudioFrame, Packet, Samples};
use crossfade_core::{Decoder, Error, SampleFormat, StreamInfo};

pub fn decoder(stream: &StreamInfo) -> Result<Box<dyn Decoder>, Error> {
    let _ = SampleFormat::F32;
    Err(Error::Unsupported {
        what: format!("no decoder for {}", stream.codec.name()),
    })
}

#[cfg(test)]
#[path = "mp3_tests.rs"]
mod tests;
