//! Codecs: decoders that turn packets into frames and encoders that do the
//! reverse. Built on the types in `crossfade-core`.
#![forbid(unsafe_code)]

#[cfg(feature = "aac")]
mod aac;
pub mod alac;
pub mod alac_encode;
pub mod flac;
mod layout;
pub mod mp3;
pub mod opus;
pub mod opus_encode;
pub mod pcm;
pub mod vorbis;

use crossfade_core::{AudioSpec, CodecId, Decoder, Encoder, Error, StreamInfo};

/// Knobs an encoder may honour; the default suits every codec.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EncodeOptions {
    /// FLAC: 0 (fastest) to 8 (smallest). Other codecs ignore it.
    pub compression_level: Option<u8>,
    /// FLAC: the true bit depth of samples carried left-justified in a wider container
    /// (`None`: the container's own width). Other codecs ignore it.
    pub bits_per_sample: Option<u8>,
    /// Opus: bits per second for the whole stream (`None`: [`opus_encode::default_bitrate`]).
    /// Other codecs ignore it.
    pub bitrate: Option<u32>,
    /// Opus: constant instead of variable bitrate. Other codecs ignore it.
    pub constant_bitrate: bool,
    /// Opus: the rate of the audio before it was brought to 48 kHz, recorded in the header
    /// (`None`: 48000). It changes no sample. Other codecs ignore it.
    pub input_sample_rate: Option<u32>,
    /// Opus: the true depth of the source's integer samples
    /// ([`opus_encode::source_bits_per_sample`]; `None`: a float or lossy source), the
    /// encoder's input depth as `opusenc` sets it (`OPUS_SET_LSB_DEPTH`, clamped to 8 to 24;
    /// `None` is 24). It moves the encoder's silence and noise-floor thresholds, no sample.
    /// Other codecs ignore it.
    pub source_bits_per_sample: Option<u8>,
}

/// What a thread count of 0 ("the machine's parallelism") stands for here.
pub fn auto_thread_count() -> usize {
    crossfade_flac::auto_thread_count()
}

/// Whether `codec`'s decoder can use more than the calling thread.
pub fn is_threaded(codec: CodecId) -> bool {
    matches!(codec, CodecId::Flac | CodecId::Alac)
}

/// Whether `codec`'s encoder can use more than the calling thread (ALAC's carries state from
/// packet to packet).
pub fn is_threaded_encoder(codec: CodecId) -> bool {
    codec == CodecId::Flac
}

/// `Error::Unsupported` for a codec with no decoder. Runs on the calling thread.
pub fn decoder_for(stream: &StreamInfo) -> Result<Box<dyn Decoder>, Error> {
    decoder_for_threads(stream, 1)
}

/// As [`decoder_for`], letting the decoder use up to `threads` threads (the calling thread
/// included; 0 means the machine's parallelism). What it decodes never depends on it; a
/// decoder with nothing to share out ignores it.
pub fn decoder_for_threads(stream: &StreamInfo, threads: usize) -> Result<Box<dyn Decoder>, Error> {
    if stream.codec.is_pcm() {
        return Ok(Box::new(pcm::PcmDecoder::new(stream)?));
    }
    if stream.codec == CodecId::Flac {
        return flac::decode::decoder_threads(stream, threads);
    }
    if stream.codec == CodecId::Vorbis {
        return vorbis::decoder(stream);
    }
    if stream.codec == CodecId::Alac {
        return alac::decoder_threads(stream, threads);
    }
    if stream.codec == CodecId::Mp3 {
        return mp3::decoder(stream);
    }
    if stream.codec == CodecId::Opus {
        return opus::decoder(stream);
    }
    #[cfg(feature = "aac")]
    if stream.codec == CodecId::Aac {
        return aac::decoder(stream);
    }
    #[cfg(not(feature = "aac"))]
    if stream.codec == CodecId::Aac {
        return Err(Error::Unsupported {
            what: "aac decoding is not part of this build (AAC is patent-encumbered); build with --features aac".into(),
        });
    }
    Err(Error::Unsupported {
        what: format!("no decoder for {}", stream.codec.name()),
    })
}

/// `Error::Unsupported` for a codec with no encoder. Runs on the calling thread.
pub fn encoder_for(
    codec: CodecId,
    spec: &AudioSpec,
    options: &EncodeOptions,
) -> Result<Box<dyn Encoder>, Error> {
    encoder_for_threads(codec, spec, options, 1)
}

/// As [`encoder_for`], letting the encoder use up to `threads` threads (the calling thread
/// included; 0 means the machine's parallelism). The output never depends on it, which is why
/// it is not one of the [`EncodeOptions`]; an encoder with nothing to share out ignores it.
pub fn encoder_for_threads(
    codec: CodecId,
    spec: &AudioSpec,
    options: &EncodeOptions,
    threads: usize,
) -> Result<Box<dyn Encoder>, Error> {
    if codec.is_pcm() {
        return Ok(Box::new(pcm::PcmEncoder::new(codec, spec)?));
    }
    if codec == CodecId::Flac {
        return flac::encode::encoder_threads(spec, options, threads);
    }
    if codec == CodecId::Alac {
        return alac_encode::encoder(spec, options);
    }
    if codec == CodecId::Opus {
        return opus_encode::encoder(spec, options);
    }
    Err(Error::Unsupported {
        what: format!("no encoder for {}", codec.name()),
    })
}
