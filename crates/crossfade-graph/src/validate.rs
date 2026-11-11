//! Every decision a job needs once its input is known, with no file touched. `Pipeline::open`
//! and a front end's pre-flight `validate` share this code, so they cannot disagree.

use crate::choice::{can_hold, can_write, encode_options, output_codec, share_threads};
use crate::filter::{self, ChainPlan};
use crate::job::AudioSettings;
use crate::two_pass::{Measuring, Pass, for_pass};
use crossfade_dsp::filters::gain::GainPlan;

/// Stands in for a normalising job's gain when only checking that the job builds.
const UNITY: GainPlan = GainPlan { linear: 1.0 };
use crossfade_codec::{
    EncodeOptions, auto_thread_count, decoder_for_threads, encoder_for_threads, is_threaded,
};
use crossfade_core::{
    AudioSpec, CodecId, Decoder, Encoder, Error, SampleFormat, StreamInfo, Tags, TimeBase,
};
use crossfade_dsp::chain::Chain;
use crossfade_dsp::dither::Dither;
use crossfade_dsp::filters::resample::{Quality, ResamplePlan};
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

/// The codecs a transcode runs, and its chain when it processes; a copy has none.
pub(crate) struct Codecs {
    pub decoder: Box<dyn Decoder>,
    pub encoder: Box<dyn Encoder>,
    pub chain: Option<Chain>,
    /// A measuring pass's meter (its encoder writes nothing).
    pub measuring: Option<Measuring>,
}

/// The true bit depth of what a transcode writes: the input's while the sample format stays as
/// it is, the full width of the new format once the format changes (`None` means full width).
/// Stream copy keeps the input's stream whole and never asks.
pub(crate) fn output_depth(input: &StreamInfo, out_format: SampleFormat) -> Option<u8> {
    input
        .bits_per_raw_sample
        .filter(|_| out_format == input.audio.format())
}

/// Whether writing `input` as `out_format` without any processing loses real bits: an integer
/// format narrower than the input's true depth (`bits_per_raw_sample`, else its format's
/// width), or float to integer. Such a conversion is dithered (unless the job says `none`).
/// Widening, the same depth and anything to float lose nothing and stay as they are.
pub(crate) fn loses_depth(input: &StreamInfo, out_format: SampleFormat) -> bool {
    let in_format = input.audio.format();
    if out_format.is_float() {
        return false;
    }
    if in_format.is_float() {
        return true;
    }
    let depth = input
        .bits_per_raw_sample
        .map_or(in_format.bits(), usize::from);
    out_format.bits() < depth
}

/// The chain a job runs on `input`, or `None` for the unprocessed path: the filters and the
/// output's rate and channel conversions (`filter::plan`), or else, when the output format
/// loses real depth and the job dithers, an empty chain that only quantises.
pub fn processing(
    input: &StreamInfo,
    settings: &AudioSettings,
) -> Result<Option<ChainPlan>, Error> {
    let chain = filter::plan(&input.audio, settings)?;
    if chain.is_some() || settings.dither == Dither::None {
        return Ok(chain);
    }
    let out_format = settings.sample_format.unwrap_or(input.audio.format());
    Ok(loses_depth(input, out_format).then(|| ChainPlan {
        filters: Vec::new(),
        dither: settings.dither,
    }))
}

/// The container named by `path`'s extension, if we can write it.
pub(crate) fn output_format_for(path: &str) -> Result<FormatId, Error> {
    let ext = path.rsplit_once('.').map_or("", |(_, e)| e);
    match FormatId::from_extension(ext) {
        Some(f) if can_write(f) => Ok(f),
        _ => Err(unsupported(format!(
            "output extension {ext:?} of {path} (can write .wav, .flac, .aiff/.aif/.aifc, \
                 .oga/.ogg)"
        ))),
    }
}

/// What the output will be: copy or transcode, the output stream, and the chain when one
/// runs. Pure: nothing is built.
fn plan_output(
    input: &StreamInfo,
    output_format: FormatId,
    settings: &AudioSettings,
) -> Result<(OutputPlan, Option<ChainPlan>), Error> {
    let requested = settings.codec.as_deref();
    if requested == Some("copy") {
        refuse_processing_on_copy(input, settings)?;
        if settings.sample_format.is_some() {
            return Err(unsupported("--codec copy cannot change the sample format"));
        }
        if settings.compression_level.is_some() {
            return Err(unsupported("--codec copy cannot take a compression level"));
        }
        if !can_hold(output_format, input.codec) {
            let why = if output_format == FormatId::Ogg {
                " (ogg holds flac only)"
            } else {
                ""
            };
            return Err(unsupported(format!(
                "cannot copy {} into {}{why}",
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
    let chain = processing(input, settings)?;
    let plan = output_plan(input, output_format, settings, chain.as_ref())?;
    Ok((plan, chain))
}

/// The output stream of a transcode of `input` through `chain` (`None`: unprocessed).
fn output_plan(
    input: &StreamInfo,
    output_format: FormatId,
    settings: &AudioSettings,
    chain: Option<&ChainPlan>,
) -> Result<OutputPlan, Error> {
    let in_spec = &input.audio;
    let out_format = settings.sample_format.unwrap_or(in_spec.format());
    let codec = output_codec(output_format, out_format, settings.codec.as_deref())?;
    let (out_spec, frames, depth, time_base) = match chain {
        // Processed audio uses every bit of its format.
        Some(c) => {
            let (s, frames) = c.output(in_spec, input.frames)?;
            let time_base = if s.sample_rate() == in_spec.sample_rate() {
                input.time_base
            } else {
                TimeBase::new(1, i64::from(s.sample_rate()))?
            };
            let spec = AudioSpec::new(s.sample_rate(), out_format, s.layout().clone())?;
            (spec, frames, None, time_base)
        }
        None => (
            AudioSpec::new(in_spec.sample_rate(), out_format, in_spec.layout().clone())?,
            input.frames,
            output_depth(input, out_format),
            input.time_base,
        ),
    };
    // The codec's options are checked here too, so a pure plan refuses what a build would.
    encode_options(codec, settings.compression_level)?;
    let output = StreamInfo {
        frames,
        bits_per_raw_sample: depth,
        ..StreamInfo::new(0, codec, out_spec, time_base)
    };
    Ok(OutputPlan {
        output_format,
        output,
        copy: false,
    })
}

/// Copying passes packets through untouched, so it cannot honour anything that changes them.
fn refuse_processing_on_copy(input: &StreamInfo, settings: &AudioSettings) -> Result<(), Error> {
    let why = "(copying passes the packets through untouched; leave out --codec copy to \
               process the audio)";
    if !settings.filters.is_empty() {
        return Err(unsupported(format!(
            "--codec copy cannot apply filters {why}"
        )));
    }
    if settings
        .sample_rate
        .is_some_and(|r| r != input.audio.sample_rate())
    {
        return Err(unsupported(format!(
            "--codec copy cannot change the sample rate {why}"
        )));
    }
    let layout = input.audio.layout();
    if settings
        .channels
        .as_ref()
        .is_some_and(|c| c.resolve(layout) != *layout)
    {
        return Err(unsupported(format!(
            "--codec copy cannot change the channels {why}"
        )));
    }
    Ok(())
}

/// Copy or transcode: the output stream, and the codecs and chain when there is a transcode,
/// sharing `threads` between the codecs (see `Limits::max_threads` and `share_threads`).
pub(crate) fn resolve(
    input: &StreamInfo,
    output_format: FormatId,
    settings: &AudioSettings,
    threads: usize,
    pass: Pass,
) -> Result<(OutputPlan, Option<Codecs>), Error> {
    let (plan, chain) = plan_output(input, output_format, settings)?;
    if plan.copy {
        return Ok((plan, None));
    }
    let (chain, measuring) = for_pass(chain, &input.audio, pass)?;
    let out = &plan.output;
    let options = EncodeOptions {
        bits_per_sample: out.bits_per_raw_sample,
        ..encode_options(out.codec, settings.compression_level)?
    };
    let (decoder_threads, encoder_threads) = codec_threads(threads, input.codec, out.codec);
    let chain_format = if measuring.is_some() {
        SampleFormat::F64
    } else {
        out.audio.format()
    };
    let chain = match &chain {
        Some(c) => Some(Chain::new(&input.audio, c, chain_format)?),
        None => None,
    };
    let (encoder, measuring) = match measuring {
        Some((encoder, m)) => (encoder, Some(m)),
        None => (
            encoder_for_threads(out.codec, &out.audio, &options, encoder_threads)?,
            None,
        ),
    };
    let codecs = Codecs {
        decoder: decoder_for_threads(input, decoder_threads)?,
        encoder,
        chain,
        measuring,
    };
    Ok((plan, Some(codecs)))
}

/// Threads for a decoder and an encoder from one budget (see `Limits::max_threads`).
pub(crate) fn codec_threads(threads: usize, input: CodecId, output: CodecId) -> (usize, usize) {
    match (threads, (is_threaded(input), is_threaded(output))) {
        // Alone, a codec on an automatic budget picks its own count (a decoder needs fewer).
        (0, (true, false)) => (0, 1),
        (0, (false, true)) => (1, 0),
        (0, (d, e)) => share_threads(auto_thread_count(), d, e),
        (n, (d, e)) => share_threads(n, d, e),
    }
}

/// What a job will write, from the input stream and the settings alone, without checking
/// that it can be built: an output description for a front end to show. [`validate`] says
/// whether it can run.
pub fn describe(
    input: &StreamInfo,
    output_path: &str,
    settings: &AudioSettings,
) -> Result<OutputPlan, Error> {
    let format = output_format_for(output_path)?;
    plan_output(input, format, settings).map(|(plan, _)| plan)
}

/// Checks a job's output against its input stream without touching any file: the container from
/// the extension, the copy rules, the codec and its options, the filters (each must be built),
/// and that the encoder and the container's muxer accept the result (FLAC's channel limit shows
/// up here).
pub fn validate(
    input: &StreamInfo,
    output_path: &str,
    settings: &AudioSettings,
) -> Result<OutputPlan, Error> {
    let format = output_format_for(output_path)?;
    // The encoder is only looked at, never fed, so it stays on this thread. A normalising
    // job's gain is not known until it runs: any gain builds the same chain.
    let (plan, _) = resolve(input, format, settings, 1, Pass::Convert(Some(UNITY)))?;
    // A muxer writes its header on creation, so a scratch buffer shows whether it would refuse.
    create_muxer(
        format,
        Box::new(Cursor::new(Vec::new())),
        &plan.output,
        &Tags::new(),
    )?;
    Ok(plan)
}

/// The stream several inputs make once joined, as the filters see it: the first input's rate
/// and layout unless the settings name others (every input is conformed to it), the first
/// input's sample format, and a known length only when every input's is known.
fn joined(inputs: &[StreamInfo], settings: &AudioSettings) -> Result<StreamInfo, Error> {
    let [first, ..] = inputs else {
        return Err(unsupported("concat needs at least one input"));
    };
    if settings.codec.as_deref() == Some("copy") {
        return Err(unsupported(
            "--codec copy cannot join inputs (joining decodes them)",
        ));
    }
    let first_spec = &first.audio;
    let rate = settings.sample_rate.unwrap_or(first_spec.sample_rate());
    let layout = settings.channels.as_ref().map_or_else(
        || first_spec.layout().clone(),
        |c| c.resolve(first_spec.layout()),
    );
    let frames = inputs.iter().try_fold(0u64, |sum, i| {
        let n = if i.audio.sample_rate() == rate {
            i.frames
        } else {
            ResamplePlan {
                rate,
                quality: Quality::default(),
            }
            .output_frames(i.audio.sample_rate(), i.frames)
        };
        sum.checked_add(n?)
    });
    // The true depth carries through only when every input is the same and none is conformed.
    let alike = inputs.iter().all(|i| {
        i.audio.sample_rate() == rate
            && *i.audio.layout() == layout
            && i.audio.format() == first_spec.format()
            && i.bits_per_raw_sample == first.bits_per_raw_sample
    });
    Ok(StreamInfo {
        frames,
        bits_per_raw_sample: first.bits_per_raw_sample.filter(|_| alike),
        ..StreamInfo::new(
            0,
            first.codec,
            AudioSpec::new(rate, first_spec.format(), layout)?,
            TimeBase::new(1, i64::from(rate))?,
        )
    })
}

/// How a join runs: each input's own conversion to the joined rate and layout (`None` when
/// it already has them), and the main chain over the joined stream.
pub(crate) struct Join {
    pub plan: OutputPlan,
    /// The joined stream as the main chain sees it.
    pub joined: StreamInfo,
    pub conform: Vec<Option<ChainPlan>>,
    pub main: Option<ChainPlan>,
}

/// Plans a join. Inputs alike (rate, layout, and none losing depth in the output format) with
/// no filters take the unprocessed path: the join is then bit-exact. Otherwise one main chain
/// runs over the whole joined stream, so dither is one stream too.
pub(crate) fn plan_join(
    inputs: &[StreamInfo],
    output_format: FormatId,
    settings: &AudioSettings,
) -> Result<Join, Error> {
    let joined = joined(inputs, settings)?;
    let conform: Vec<Option<ChainPlan>> = inputs
        .iter()
        .map(|i| {
            let from = AudioSpec::new(
                i.audio.sample_rate(),
                SampleFormat::F64,
                i.audio.layout().clone(),
            )?;
            let filters = filter::conversions(
                &from,
                joined.audio.sample_rate(),
                joined.audio.layout().clone(),
            );
            Ok((!filters.is_empty()).then_some(ChainPlan {
                filters,
                dither: Dither::None,
            }))
        })
        .collect::<Result<_, Error>>()?;
    let mut main = processing(&joined, settings)?;
    let out_format = settings.sample_format.unwrap_or(joined.audio.format());
    let narrows =
        settings.dither != Dither::None && inputs.iter().any(|i| loses_depth(i, out_format));
    if main.is_none() && (narrows || conform.iter().any(Option::is_some)) {
        main = Some(ChainPlan {
            filters: Vec::new(),
            dither: settings.dither,
        });
    }
    let plan = output_plan(&joined, output_format, settings, main.as_ref())?;
    Ok(Join {
        plan,
        joined,
        conform,
        main,
    })
}

/// The built parts of a join that live for the whole job; each input's decoder and
/// conversion are built when it is opened.
pub(crate) struct JoinCodecs {
    pub encoder: Box<dyn Encoder>,
    pub main: Option<Chain>,
    pub measuring: Option<Measuring>,
    /// Per input.
    pub decoder_threads: Vec<usize>,
}

/// [`plan_join`], then builds what the job needs and checks each input's conversion builds.
pub(crate) fn resolve_join(
    inputs: &[StreamInfo],
    output_format: FormatId,
    settings: &AudioSettings,
    threads: usize,
    pass: Pass,
) -> Result<(Join, JoinCodecs), Error> {
    let mut join = plan_join(inputs, output_format, settings)?;
    let (main, measuring) = for_pass(join.main.take(), &join.joined.audio, pass)?;
    join.main = main;
    for (input, c) in inputs.iter().zip(&join.conform) {
        if let Some(c) = c {
            Chain::new(&input.audio, c, SampleFormat::F64)?;
        }
    }
    let out = &join.plan.output;
    // Decoders run one at a time, so each may share the budget with the encoder alone.
    let decoder_threads = inputs
        .iter()
        .map(|i| codec_threads(threads, i.codec, out.codec).0)
        .collect();
    let encoder_threads = codec_threads(threads, inputs[0].codec, out.codec).1;
    let options = EncodeOptions {
        bits_per_sample: out.bits_per_raw_sample,
        ..encode_options(out.codec, settings.compression_level)?
    };
    let chain_format = if measuring.is_some() {
        SampleFormat::F64
    } else {
        out.audio.format()
    };
    let main = match &join.main {
        Some(c) => Some(Chain::new(&join.joined.audio, c, chain_format)?),
        None => None,
    };
    let (encoder, measuring) = match measuring {
        Some((encoder, m)) => (encoder, Some(m)),
        None => (
            encoder_for_threads(out.codec, &out.audio, &options, encoder_threads)?,
            None,
        ),
    };
    Ok((
        join,
        JoinCodecs {
            encoder,
            main,
            measuring,
            decoder_threads,
        },
    ))
}

/// [`describe`] for a job that joins `inputs` in order (`crossfade concat`).
pub fn describe_concat(
    inputs: &[StreamInfo],
    output_path: &str,
    settings: &AudioSettings,
) -> Result<OutputPlan, Error> {
    let format = output_format_for(output_path)?;
    plan_join(inputs, format, settings).map(|j| j.plan)
}

/// [`validate`] for a job that joins `inputs` in order: everything `Pipeline::open` decides,
/// with no file touched.
pub fn validate_concat(
    inputs: &[StreamInfo],
    output_path: &str,
    settings: &AudioSettings,
) -> Result<OutputPlan, Error> {
    let format = output_format_for(output_path)?;
    let (join, _) = resolve_join(inputs, format, settings, 1, Pass::Convert(Some(UNITY)))?;
    create_muxer(
        format,
        Box::new(Cursor::new(Vec::new())),
        &join.plan.output,
        &Tags::new(),
    )?;
    Ok(join.plan)
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
