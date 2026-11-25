//! The one rule for the codecs that store a single layout per channel count (ALAC, and Opus
//! from three channels up): what a source layout needs to be written as theirs.
//!
//! [`ChannelLayout::fit_to`] decides. A source that is the codec's layout, names its single
//! surround pair side where the codec says back (or the reverse) or has no labels is written
//! in place, channel for channel; the second and third get a note. Anything else is a problem.

use crossfade_core::{Channel, ChannelLayout, Fit};

/// `quad with side surrounds`, `5.1 with back surrounds`, else the layout's name.
fn words(layout: &ChannelLayout) -> String {
    let named = layout.name().and_then(|n| {
        let (kind, pair) = match n {
            "quad" => ("quad", "back"),
            "quad-side" => ("quad", "side"),
            "5.0" => ("5.0", "side"),
            "5.0-back" => ("5.0", "back"),
            "5.1" => ("5.1", "side"),
            "5.1-back" => ("5.1", "back"),
            _ => return None,
        };
        Some(format!("{kind} with {pair} surrounds"))
    });
    named
        .or_else(|| layout.name().map(str::to_owned))
        .unwrap_or_else(|| format!("{} channels", layout.channels()))
}

/// What a front end should say when `source` is written as `wanted` (the codec's layout for its
/// count) by assumption: a renamed surround pair or unlabelled channels. `None` when it is the
/// codec's own layout or cannot be written.
pub(crate) fn note(codec: &str, source: &ChannelLayout, wanted: &ChannelLayout) -> Option<String> {
    let n = source.channels();
    match source.fit_to(wanted) {
        Fit::Renamed => {
            let back = wanted
                .positions()
                .is_some_and(|p| p.contains(&Channel::BackLeft));
            let pair = if back { "back" } else { "side" };
            Some(format!(
                "{} written as {}: {codec} names its surround pair as {pair} speakers",
                words(source),
                words(wanted)
            ))
        }
        Fit::Unlabelled => Some(match wanted.name() {
            Some(_) => format!(
                "{n} unlabelled channels taken as {}: {codec}'s layout for {n} channels",
                words(wanted)
            ),
            None => format!("{n} unlabelled channels taken as {codec}'s own {n}-channel layout"),
        }),
        Fit::Same | Fit::Different => None,
    }
}

/// What is wrong with writing `source` as `wanted`, if anything: it is none of the three that
/// [`ChannelLayout::fit_to`] accepts.
pub(crate) fn problem(
    codec: &str,
    source: &ChannelLayout,
    wanted: &ChannelLayout,
) -> Option<String> {
    if source.fit_to(wanted) != Fit::Different {
        return None;
    }
    let n = source.channels();
    let have = if source.positions().is_none() {
        format!("{n} unlabelled channels")
    } else {
        source
            .name()
            .map_or_else(|| format!("{n} channels in another layout"), str::to_owned)
    };
    Some(format!(
        "{codec} stores {n} channels in one layout only ({}) and this stream is {have}",
        wanted
            .name()
            .map_or_else(|| format!("its own {n}-channel order"), str::to_owned)
    ))
}
