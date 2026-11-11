//! From the user's words to what the chain runs: the one mapping from [`FilterSpec`] (units,
//! seconds, dB) to `FilterPlan` (frames at each filter's own rate, linear gains), plus the
//! conversions the output settings ask for, appended at the end.

use super::FilterSpec;
use crate::job::AudioSettings;
use crossfade_core::{AudioSpec, ChannelLayout, Error, SampleFormat};
use crossfade_dsp::chain::ChainPlan;
use crossfade_dsp::filters::FilterPlan;
use crossfade_dsp::filters::fade::FadePlan;
use crossfade_dsp::filters::gain::GainPlan;
use crossfade_dsp::filters::mix::MixPlan;
use crossfade_dsp::filters::normalize::NormalizePlan;
use crossfade_dsp::filters::resample::{Quality, ResamplePlan};
use crossfade_dsp::filters::trim::TrimPlan;
use crossfade_dsp::level::db_to_linear;

/// The chain for `settings` on `input`, or `None` when nothing asks for processing (no
/// filters, and the output rate and channels are the input's): that job takes the
/// unprocessed, bit-exact path.
pub fn plan(input: &AudioSpec, settings: &AudioSettings) -> Result<Option<ChainPlan>, Error> {
    settings.check().map_err(|e| Error::Malformed {
        what: e.to_string(),
    })?;
    let mut spec = AudioSpec::new(
        input.sample_rate(),
        SampleFormat::F64,
        input.layout().clone(),
    )?;
    let mut filters = Vec::with_capacity(settings.filters.len() + 2);
    for f in &settings.filters {
        let p = plan_filter(f, &spec);
        spec = p.output_spec(&spec)?;
        filters.push(p);
    }
    let normalizers = filters
        .iter()
        .filter(|f| matches!(f, FilterPlan::Normalize(_)))
        .count();
    if normalizers > 1 {
        return Err(Error::Unsupported {
            what: format!(
                "{normalizers} normalize filters (a chain takes one: each needs its own \
                 measuring pass)"
            ),
        });
    }
    let rate = settings.sample_rate.unwrap_or(spec.sample_rate());
    let layout = settings
        .channels
        .as_ref()
        .map_or_else(|| spec.layout().clone(), |c| c.resolve(spec.layout()));
    filters.extend(conversions(&spec, rate, layout));
    if filters.is_empty() {
        return Ok(None);
    }
    Ok(Some(ChainPlan {
        filters,
        dither: settings.dither,
    }))
}

/// The resample and mix stages that take `from` to `rate` and `layout`, if any. Fewer channels
/// to resample is cheaper: mix down first, mix up last.
pub(crate) fn conversions(from: &AudioSpec, rate: u32, layout: ChannelLayout) -> Vec<FilterPlan> {
    let resample = (rate != from.sample_rate()).then_some(FilterPlan::Resample(ResamplePlan {
        rate,
        quality: Quality::default(),
    }));
    let down = layout.channels() < from.layout().channels();
    let mix = (layout != *from.layout()).then_some(FilterPlan::Mix(MixPlan {
        layout,
        matrix: None,
    }));
    let order = if down {
        [mix, resample]
    } else {
        [resample, mix]
    };
    order.into_iter().flatten().collect()
}

/// One filter at the rate and layout it will see.
fn plan_filter(spec: &FilterSpec, at: &AudioSpec) -> FilterPlan {
    // Times are rounded to the nearest frame; `check` keeps them small enough to be exact.
    let frames = |seconds: f64| (seconds * f64::from(at.sample_rate())).round() as u64;
    match spec {
        FilterSpec::Gain { db } => FilterPlan::Gain(GainPlan {
            linear: db_to_linear(*db),
        }),
        FilterSpec::Trim {
            start_seconds,
            end_seconds,
            duration_seconds,
        } => {
            let start = start_seconds.map_or(0, frames);
            let end = end_seconds
                .map(frames)
                .or_else(|| duration_seconds.map(|d| start + frames(d)));
            FilterPlan::Trim(TrimPlan { start, end })
        }
        FilterSpec::Fade {
            in_seconds,
            out_seconds,
            curve,
        } => FilterPlan::Fade(FadePlan {
            fade_in: in_seconds.map_or(0, frames),
            fade_out: out_seconds.map_or(0, frames),
            curve: *curve,
        }),
        FilterSpec::Normalize {
            lufs,
            true_peak_dbtp,
        } => FilterPlan::Normalize(NormalizePlan {
            target_lufs: *lufs,
            ceiling_dbtp: *true_peak_dbtp,
        }),
        FilterSpec::Resample { rate, quality } => FilterPlan::Resample(ResamplePlan {
            rate: *rate,
            quality: *quality,
        }),
        FilterSpec::Mix { channels, matrix } => FilterPlan::Mix(MixPlan {
            layout: channels.resolve(at.layout()),
            matrix: matrix.clone(),
        }),
    }
}
