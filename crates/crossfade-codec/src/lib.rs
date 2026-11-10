//! Codecs: decoders that turn packets into frames and encoders that do the
//! reverse. Built on the types in `crossfade-core`.
#![forbid(unsafe_code)]

pub mod flac;
pub mod pcm;

use crossfade_core::{AudioSpec, CodecId, Decoder, Encoder, Error, StreamInfo};

/// Knobs an encoder may honour; the default suits every codec.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EncodeOptions {
    /// FLAC: 0 (fastest) to 8 (smallest). Other codecs ignore it.
    pub compression_level: Option<u8>,
}

/// `Error::Unsupported` for a codec with no decoder.
pub fn decoder_for(stream: &StreamInfo) -> Result<Box<dyn Decoder>, Error> {
    if stream.codec.is_pcm() {
        return Ok(Box::new(pcm::PcmDecoder::new(stream)?));
    }
    if stream.codec == CodecId::Flac {
        return flac::decode::decoder(stream);
    }
    Err(Error::Unsupported {
        what: format!("no decoder for {}", stream.codec.name()),
    })
}

/// `Error::Unsupported` for a codec with no encoder.
pub fn encoder_for(
    codec: CodecId,
    spec: &AudioSpec,
    options: &EncodeOptions,
) -> Result<Box<dyn Encoder>, Error> {
    if codec.is_pcm() {
        return Ok(Box::new(pcm::PcmEncoder::new(codec, spec)?));
    }
    if codec == CodecId::Flac {
        return flac::encode::encoder(spec, options);
    }
    Err(Error::Unsupported {
        what: format!("no encoder for {}", codec.name()),
    })
}
