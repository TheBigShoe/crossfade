//! Every decision a job needs once its input is known, with no file touched. `Pipeline::open`
//! and a front end's pre-flight `validate` share this code, so they cannot disagree.

use crate::choice::{can_hold, can_write, encode_options, output_codec, share_threads};
use crate::job::AudioSettings;
use crossfade_codec::{
    EncodeOptions, auto_thread_count, decoder_for_threads, encoder_for_threads, is_threaded,
};
use crossfade_core::{AudioSpec, Decoder, Encoder, Error, SampleFormat, StreamInfo, Tags};
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

/// The true bit depth of what a transcode writes: the input's while the sample format stays as
/// it is, the full width of the new format once the format changes (`None` means full width).
/// Stream copy keeps the input's stream whole and never asks.
pub(crate) fn output_depth(input: &StreamInfo, out_format: SampleFormat) -> Option<u8> {
    input
        .bits_per_raw_sample
        .filter(|_| out_format == input.audio.format())
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

/// Copy or transcode: the output stream, and the codecs when there is a transcode, sharing
/// `threads` between them (see `Limits::max_threads` and `share_threads`).
pub(crate) fn resolve(
    input: &StreamInfo,
    output_format: FormatId,
    settings: &AudioSettings,
    threads: usize,
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
    let depth = output_depth(input, out_format);
    let options = EncodeOptions {
        bits_per_sample: depth,
        ..encode_options(codec, settings.compression_level)?
    };
    let out_spec = AudioSpec::new(in_spec.sample_rate(), out_format, in_spec.layout().clone())?;
    let output = StreamInfo {
        frames: input.frames,
        bits_per_raw_sample: depth,
        ..StreamInfo::new(0, codec, out_spec.clone(), input.time_base)
    };
    let threaded = (is_threaded(input.codec), is_threaded(codec));
    let (decoder_threads, encoder_threads) = match (threads, threaded) {
        // Alone, a codec on an automatic budget picks its own count (a decoder needs fewer).
        (0, (true, false)) => (0, 1),
        (0, (false, true)) => (1, 0),
        (0, (d, e)) => share_threads(auto_thread_count(), d, e),
        (n, (d, e)) => share_threads(n, d, e),
    };
    let codecs = Codecs {
        decoder: decoder_for_threads(input, decoder_threads)?,
        encoder: encoder_for_threads(codec, &out_spec, &options, encoder_threads)?,
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
    // The encoder is only looked at, never fed, so it stays on this thread.
    let (plan, _) = resolve(input, format, settings, 1)?;
    // A muxer writes its header on creation, so a scratch buffer shows whether it would refuse.
    create_muxer(
        format,
        Box::new(Cursor::new(Vec::new())),
        &plan.output,
        &Tags::new(),
    )?;
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossfade_core::{ChannelLayout, CodecId, TimeBase};

    fn stream(format: SampleFormat, raw: Option<u8>) -> StreamInfo {
        let audio = AudioSpec::new(48000, format, ChannelLayout::stereo()).unwrap();
        StreamInfo {
            bits_per_raw_sample: raw,
            ..StreamInfo::new(
                0,
                CodecId::pcm_for(format),
                audio,
                TimeBase::new(1, 48000).unwrap(),
            )
        }
    }

    fn depth_of(input: &StreamInfo, path: &str, format: Option<SampleFormat>) -> Option<u8> {
        let settings = AudioSettings {
            sample_format: format,
            ..AudioSettings::default()
        };
        validate(input, path, &settings)
            .unwrap()
            .output
            .bits_per_raw_sample
    }

    #[test]
    fn the_depth_carries_through_while_the_format_is_unchanged() {
        use SampleFormat::*;
        let twenty = stream(S24, Some(20));
        for path in ["o.wav", "o.flac", "o.aiff"] {
            assert_eq!(depth_of(&twenty, path, None), Some(20), "{path}");
            assert_eq!(depth_of(&twenty, path, Some(S24)), Some(20), "{path}");
        }
        assert_eq!(depth_of(&stream(S16, Some(12)), "o.flac", None), Some(12));
        assert_eq!(depth_of(&stream(U8, Some(6)), "o.wav", None), Some(6));
        assert_eq!(depth_of(&stream(S16, None), "o.flac", None), None);
    }

    #[test]
    fn a_requested_format_gets_its_full_width() {
        use SampleFormat::*;
        let twenty = stream(S24, Some(20));
        for to in [S16, S32, U8, F32] {
            let path = if to.is_float() { "o.wav" } else { "o.flac" };
            assert_eq!(depth_of(&twenty, path, Some(to)), None, "{to:?}");
        }
        assert_eq!(depth_of(&stream(S16, None), "o.flac", Some(S24)), None);
    }

    #[test]
    fn stream_copy_never_changes_it() {
        let settings = AudioSettings {
            codec: Some("copy".into()),
            ..AudioSettings::default()
        };
        let mut flac = stream(SampleFormat::S24, Some(20));
        flac.codec = CodecId::Flac;
        let plan = validate(&flac, "o.flac", &settings);
        // A synthesised STREAMINFO is enough for the muxer; the plan keeps the stream as is.
        assert_eq!(plan.unwrap().output.bits_per_raw_sample, Some(20));
    }
}
