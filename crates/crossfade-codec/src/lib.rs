//! Codecs: decoders that turn packets into frames and encoders that do the
//! reverse. Built on the types in `crossfade-core`.
#![forbid(unsafe_code)]

pub mod bits;
pub mod md5;
pub mod pcm;

use crossfade_core::{AudioSpec, CodecId, Decoder, Encoder, Error, StreamInfo};

/// `Error::Unsupported` for a codec with no decoder.
pub fn decoder_for(stream: &StreamInfo) -> Result<Box<dyn Decoder>, Error> {
    if stream.codec.pcm_sample_format().is_some() {
        return Ok(Box::new(pcm::PcmDecoder::new(stream)?));
    }
    Err(Error::Unsupported {
        what: format!("no decoder for {}", stream.codec.name()),
    })
}

/// `Error::Unsupported` for a codec with no encoder.
pub fn encoder_for(codec: CodecId, spec: &AudioSpec) -> Result<Box<dyn Encoder>, Error> {
    if codec.pcm_sample_format().is_some() {
        return Ok(Box::new(pcm::PcmEncoder::new(codec, spec)?));
    }
    Err(Error::Unsupported {
        what: format!("no encoder for {}", codec.name()),
    })
}
