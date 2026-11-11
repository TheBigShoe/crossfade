//! Every decision a job needs once its input is known, with no file touched. `Pipeline::open`
//! and a front end's pre-flight `validate` share this code, so they cannot disagree.

use crate::choice::{can_hold, can_write, encode_options, output_codec};
use crate::job::AudioSettings;
use crossfade_codec::{decoder_for, encoder_for};
use crossfade_core::{AudioSpec, Decoder, Encoder, Error, StreamInfo, Tags};
use crossfade_format::{FormatId, create_muxer};
use std::io::Cursor;

fn unsupported(what: impl Into<String>) -> Error {
    Error::Unsupported { what: what.into() }
}

/// What a job will write, decided from the input stream and the settings alone.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputPlan {
    pub output_format: FormatId,
    pub output: StreamInfo,
    /// Packets go straight from demuxer to muxer.
    pub copy: bool,
}

/// The codecs a transcode runs; a copy has none.
pub(crate) struct Codecs {
    pub decoder: Box<dyn Decoder>,
    pub encoder: Box<dyn Encoder>,
}

/// The container named by `path`'s extension, if we can write it.
pub(crate) fn output_format_for(path: &str) -> Result<FormatId, Error> {
    let ext = path.rsplit_once('.').map_or("", |(_, e)| e);
    match FormatId::from_extension(ext) {
        Some(f) if can_write(f) => Ok(f),
        _ => Err(unsupported(format!(
            "output extension {ext:?} of {path} (can write .wav, .flac, .aiff/.aif/.aifc; \
                 .ogg can be read but not written yet)"
        ))),
    }
}

/// Copy or transcode: the output stream, and the codecs when there is a transcode.
pub(crate) fn resolve(
    input: &StreamInfo,
    output_format: FormatId,
    settings: &AudioSettings,
) -> Result<(OutputPlan, Option<Codecs>), Error> {
    let requested = settings.codec.as_deref();
    if requested == Some("copy") {
        if settings.sample_format.is_some() {
            return Err(unsupported("--codec copy cannot change the sample format"));
        }
        if settings.compression_level.is_some() {
            return Err(unsupported("--codec copy cannot take a compression level"));
        }
        if !can_hold(output_format, input.codec) {
            return Err(unsupported(format!(
                "cannot copy {} into {}",
                input.codec.name(),
                output_format.name()
            )));
        }
        let output = StreamInfo {
            index: 0,
            ..input.clone()
        };
        let plan = OutputPlan {
            output_format,
            output,
            copy: true,
        };
        return Ok((plan, None));
    }
    let in_spec = &input.audio;
    let out_format = settings.sample_format.unwrap_or(in_spec.format());
    let codec = output_codec(output_format, out_format, requested)?;
    let options = encode_options(codec, settings.compression_level)?;
    let out_spec = AudioSpec::new(in_spec.sample_rate(), out_format, in_spec.layout().clone())?;
    let output = StreamInfo {
        frames: input.frames,
        // Only meaningful while the sample format is unchanged.
        bits_per_raw_sample: input
            .bits_per_raw_sample
            .filter(|_| out_format == in_spec.format()),
        ..StreamInfo::new(0, codec, out_spec.clone(), input.time_base)
    };
    let codecs = Codecs {
        decoder: decoder_for(input)?,
        encoder: encoder_for(codec, &out_spec, &options)?,
    };
    let plan = OutputPlan {
        output_format,
        output,
        copy: false,
    };
    Ok((plan, Some(codecs)))
}

/// Checks a job's output against its input stream without touching any file: the container from
/// the extension, the copy rules, the codec and its options, and that the encoder and the
/// container's muxer accept the result (FLAC's channel limit shows up here).
pub fn validate(
    input: &StreamInfo,
    output_path: &str,
    settings: &AudioSettings,
) -> Result<OutputPlan, Error> {
    let format = output_format_for(output_path)?;
    let (plan, _) = resolve(input, format, settings)?;
    // A muxer writes its header on creation, so a scratch buffer shows whether it would refuse.
    create_muxer(
        format,
        Box::new(Cursor::new(Vec::new())),
        &plan.output,
        &Tags::new(),
    )?;
    Ok(plan)
}
