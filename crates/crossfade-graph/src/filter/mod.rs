//! Filters as the user writes them: the text form (`gain -6dB`, `trim start=10s end=1:30`),
//! the structured form a job stores, and the rules both obey. Units live here; the running
//! form (frames, linear gains) is `crossfade_dsp`'s `FilterPlan`, and [`plan`] is the one
//! mapping from one to the other.
//!
//! Text grammar: words separated by spaces; the first is the filter's name, an optional plain
//! value follows it, then `key=value` words in any order, each key at most once. `Display`
//! writes the canonical form, which parses back to the same value.

mod plan;
mod words;

pub use crossfade_dsp::chain::ChainPlan;
pub use crossfade_dsp::filters::FilterPlan;
pub use plan::plan;

pub use crossfade_dsp::filters::fade::Curve;
pub use crossfade_dsp::filters::resample::Quality;
pub use words::{
    MAX_CHANNELS, MAX_RATE, MAX_SECONDS, parse_channels, parse_db, parse_dbtp, parse_lufs,
    parse_rate, parse_time,
};

use crossfade_core::ChannelLayout;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// What went wrong in a filter's text or data, naming the offending word when there is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterError(String);

impl FilterError {
    pub(crate) fn at(word: &str, message: impl fmt::Display) -> Self {
        Self(format!("\"{word}\": {message}"))
    }

    fn plain(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for FilterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FilterError {}

/// The channels asked for: a named layout, or a count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Channels {
    /// One of [`ChannelLayout::NAMES`].
    Layout(ChannelLayout),
    /// A count from 1 to [`MAX_CHANNELS`]: the input's own layout when it has that many
    /// channels, else FLAC's default layout for the count.
    Count(u16),
}

impl Channels {
    /// The layout this asks for, given the layout it is applied to.
    pub fn resolve(&self, input: &ChannelLayout) -> ChannelLayout {
        match self {
            Self::Layout(l) => l.clone(),
            Self::Count(n) if *n == input.channels() => input.clone(),
            Self::Count(n) => ChannelLayout::default_for(*n).unwrap_or_else(|_| input.clone()),
        }
    }

    pub fn channels(&self) -> u16 {
        match self {
            Self::Layout(l) => l.channels(),
            Self::Count(n) => *n,
        }
    }

    pub(crate) fn check(&self) -> Result<(), String> {
        let ok = match self {
            Self::Layout(l) => l.name().is_some(),
            Self::Count(n) => (1..=MAX_CHANNELS).contains(n),
        };
        if ok {
            Ok(())
        } else {
            Err(words::channels_expected())
        }
    }
}

impl fmt::Display for Channels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Layout(l) => match l.name() {
                Some(n) => f.write_str(n),
                None => write!(f, "{}", l.channels()),
            },
            Self::Count(n) => write!(f, "{n}"),
        }
    }
}

impl Serialize for Channels {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Channels {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        parse_channels(&s).map_err(serde::de::Error::custom)
    }
}

pub(crate) fn check_rate(rate: u32) -> Result<(), String> {
    if (1..=MAX_RATE).contains(&rate) {
        Ok(())
    } else {
        Err(format!("the rate must be from 1Hz to {MAX_RATE}Hz"))
    }
}

/// One filter as a job stores it. Times are in seconds, levels in dB.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "filter", rename_all = "snake_case", deny_unknown_fields)]
pub enum FilterSpec {
    /// `gain -6dB`
    Gain { db: f64 },
    /// `trim start=10s end=1:30`, or `duration=` instead of `end=`; at least one of them.
    Trim {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        start_seconds: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        end_seconds: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_seconds: Option<f64>,
    },
    /// `fade in=2s out=3s curve=equal-power`; at least one of `in` and `out`.
    Fade {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        in_seconds: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        out_seconds: Option<f64>,
        #[serde(default, skip_serializing_if = "is_linear", with = "curve_name")]
        curve: Curve,
    },
    /// `normalize -16LUFS peak=-1dBTP`
    Normalize {
        lufs: f64,
        #[serde(default = "default_peak")]
        true_peak_dbtp: f64,
    },
    /// `resample 48000 quality=best`
    Resample {
        rate: u32,
        #[serde(default, skip_serializing_if = "is_good", with = "quality_name")]
        quality: Quality,
    },
    /// `mix stereo`, or `mix 2 matrix=1,0.7/0,0.7` (rows are outputs, linear gains).
    Mix {
        channels: Channels,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        matrix: Option<Vec<Vec<f64>>>,
    },
}

/// `normalize`'s ceiling when none is given: the EBU R 128 delivery limit.
pub const DEFAULT_TRUE_PEAK_DBTP: f64 = -1.0;

fn default_peak() -> f64 {
    DEFAULT_TRUE_PEAK_DBTP
}

fn is_linear(c: &Curve) -> bool {
    *c == Curve::Linear
}

fn is_good(q: &Quality) -> bool {
    *q == Quality::Good
}

/// Serde for the dsp's plain enums, by their `name`s (dsp stays serde-free).
macro_rules! by_name {
    ($module:ident, $type:ty, $what:literal) => {
        pub(crate) mod $module {
            use super::*;

            pub(crate) fn serialize<S: Serializer>(v: &$type, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(v.name())
            }

            pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<$type, D::Error> {
                let s = String::deserialize(d)?;
                <$type>::from_name(&s).ok_or_else(|| {
                    serde::de::Error::custom(format!(
                        "\"{s}\": expected {}: {}",
                        $what,
                        one_of(&<$type>::NAMES)
                    ))
                })
            }
        }
    };
}

by_name!(curve_name, Curve, "a curve");
by_name!(quality_name, Quality, "a quality");
by_name!(dither_name, crossfade_dsp::dither::Dither, "a dither");

/// "a, b or c".
fn one_of(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

/// A rule broken by a value: `key` is the option (`""` for the plain value) whose word to
/// quote, or `None` when no single word is at fault.
struct Invalid {
    key: Option<&'static str>,
    message: String,
}

fn invalid(key: &'static str, message: impl Into<String>) -> Invalid {
    Invalid {
        key: Some(key),
        message: message.into(),
    }
}

fn missing(message: impl Into<String>) -> Invalid {
    Invalid {
        key: None,
        message: message.into(),
    }
}

const MAX_COEFFICIENT: f64 = 16.0;

fn seconds_word(x: f64) -> String {
    format!("{x}s")
}

impl FilterSpec {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Gain { .. } => "gain",
            Self::Trim { .. } => "trim",
            Self::Fade { .. } => "fade",
            Self::Normalize { .. } => "normalize",
            Self::Resample { .. } => "resample",
            Self::Mix { .. } => "mix",
        }
    }

    /// The rules on values, whether they came from text or from a job file.
    pub fn check(&self) -> Result<(), FilterError> {
        self.rules().map_err(|e| {
            // Quote the canonical word for the field at fault.
            let text = self.to_string();
            let word = e.key.and_then(|k| {
                let mut words = text.split_whitespace().skip(1);
                if k.is_empty() {
                    words.next().map(str::to_owned)
                } else {
                    words
                        .find(|w| w.split_once('=').is_some_and(|(key, _)| key == k))
                        .map(str::to_owned)
                }
            });
            match word {
                Some(w) => FilterError::at(&w, e.message),
                None => FilterError::plain(e.message),
            }
        })
    }

    fn rules(&self) -> Result<(), Invalid> {
        let time = |key, x: Option<f64>| match x {
            Some(t) => words::check_seconds(t).map_err(|m| invalid(key, m)),
            None => Ok(()),
        };
        let positive = |key, x: Option<f64>| match x {
            Some(t) if t <= 0.0 => Err(invalid(key, format!("{key} must be more than 0s"))),
            _ => Ok(()),
        };
        match self {
            Self::Gain { db } => {
                if !(-120.0..=60.0).contains(db) {
                    return Err(invalid("", "gain must be from -120dB to 60dB"));
                }
            }
            Self::Trim {
                start_seconds,
                end_seconds,
                duration_seconds,
            } => {
                if start_seconds.is_none() && end_seconds.is_none() && duration_seconds.is_none() {
                    return Err(missing("trim needs start=, end= or duration="));
                }
                time("start", *start_seconds)?;
                time("end", *end_seconds)?;
                time("duration", *duration_seconds)?;
                if end_seconds.is_some() && duration_seconds.is_some() {
                    return Err(invalid("duration", "give end or duration, not both"));
                }
                let start = start_seconds.unwrap_or(0.0);
                if let Some(end) = end_seconds
                    && *end <= start
                {
                    return Err(invalid(
                        "end",
                        format!("end must be after start ({})", seconds_word(start)),
                    ));
                }
                positive("duration", *duration_seconds)?;
                if let Some(d) = duration_seconds
                    && start + d > MAX_SECONDS
                {
                    return Err(invalid("duration", "a time must be at most 1000000000s"));
                }
            }
            Self::Fade {
                in_seconds,
                out_seconds,
                ..
            } => {
                if in_seconds.is_none() && out_seconds.is_none() {
                    return Err(missing("fade needs in= or out="));
                }
                time("in", *in_seconds)?;
                time("out", *out_seconds)?;
                positive("in", *in_seconds)?;
                positive("out", *out_seconds)?;
            }
            Self::Normalize {
                lufs,
                true_peak_dbtp,
            } => {
                if !(-70.0..=0.0).contains(lufs) {
                    return Err(invalid("", "the target must be from -70LUFS to 0LUFS"));
                }
                if !(-60.0..=0.0).contains(true_peak_dbtp) {
                    return Err(invalid("peak", "peak must be from -60dBTP to 0dBTP"));
                }
            }
            Self::Resample { rate, .. } => check_rate(*rate).map_err(|m| invalid("", m))?,
            Self::Mix { channels, matrix } => {
                channels.check().map_err(|m| invalid("", m))?;
                if let Some(m) = matrix {
                    check_matrix(m, channels).map_err(|msg| invalid("matrix", msg))?;
                }
            }
        }
        Ok(())
    }
}

fn check_matrix(m: &[Vec<f64>], channels: &Channels) -> Result<(), String> {
    let width = m.first().map_or(0, Vec::len);
    if width == 0 {
        return Err("expected rows of numbers such as 0.5,0.5/1,0".into());
    }
    if m.iter().any(|row| row.len() != width) {
        let s = if width == 1 { "" } else { "s" };
        return Err(format!(
            "every row needs {width} coefficient{s}, like the first"
        ));
    }
    let n = channels.channels();
    if m.len() != usize::from(n) {
        let (cs, rs) = if n == 1 { ("", "") } else { ("s", "s") };
        return Err(format!(
            "{channels} has {n} channel{cs}, so the matrix needs {n} row{rs}"
        ));
    }
    if m.iter()
        .flatten()
        .any(|c| !(-MAX_COEFFICIENT..=MAX_COEFFICIENT).contains(c))
    {
        return Err(format!(
            "coefficients must be from -{MAX_COEFFICIENT} to {MAX_COEFFICIENT}"
        ));
    }
    Ok(())
}

impl fmt::Display for FilterSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())?;
        let opt = |f: &mut fmt::Formatter<'_>, key, x: &Option<f64>| match x {
            Some(t) => write!(f, " {key}={}", seconds_word(*t)),
            None => Ok(()),
        };
        match self {
            Self::Gain { db } => write!(f, " {db}dB"),
            Self::Trim {
                start_seconds,
                end_seconds,
                duration_seconds,
            } => {
                opt(f, "start", start_seconds)?;
                opt(f, "end", end_seconds)?;
                opt(f, "duration", duration_seconds)
            }
            Self::Fade {
                in_seconds,
                out_seconds,
                curve,
            } => {
                opt(f, "in", in_seconds)?;
                opt(f, "out", out_seconds)?;
                if *curve != Curve::Linear {
                    write!(f, " curve={}", curve.name())?;
                }
                Ok(())
            }
            Self::Normalize {
                lufs,
                true_peak_dbtp,
            } => {
                write!(f, " {lufs}LUFS")?;
                if *true_peak_dbtp != DEFAULT_TRUE_PEAK_DBTP {
                    write!(f, " peak={true_peak_dbtp}dBTP")?;
                }
                Ok(())
            }
            Self::Resample { rate, quality } => {
                write!(f, " {rate}Hz")?;
                if *quality != Quality::Good {
                    write!(f, " quality={}", quality.name())?;
                }
                Ok(())
            }
            Self::Mix { channels, matrix } => {
                write!(f, " {channels}")?;
                if let Some(m) = matrix {
                    f.write_str(" matrix=")?;
                    for (i, row) in m.iter().enumerate() {
                        if i > 0 {
                            f.write_str("/")?;
                        }
                        for (j, c) in row.iter().enumerate() {
                            if j > 0 {
                                f.write_str(",")?;
                            }
                            write!(f, "{c}")?;
                        }
                    }
                }
                Ok(())
            }
        }
    }
}

/// What a filter's text may hold besides its name.
struct Shape {
    name: &'static str,
    /// The error when the plain value is missing; `None` when the filter takes none.
    needs: Option<&'static str>,
    keys: &'static [&'static str],
}

const SHAPES: [Shape; 6] = [
    Shape {
        name: "gain",
        needs: Some("gain needs a level, such as \"gain -3dB\""),
        keys: &[],
    },
    Shape {
        name: "trim",
        needs: None,
        keys: &["start", "end", "duration"],
    },
    Shape {
        name: "fade",
        needs: None,
        keys: &["in", "out", "curve"],
    },
    Shape {
        name: "normalize",
        needs: Some("normalize needs a target, such as \"normalize -16LUFS\""),
        keys: &["peak"],
    },
    Shape {
        name: "resample",
        needs: Some("resample needs a rate, such as \"resample 48000\""),
        keys: &["quality"],
    },
    Shape {
        name: "mix",
        needs: Some("mix needs a layout, such as \"mix stereo\""),
        keys: &["matrix"],
    },
];

impl Shape {
    fn expected_keys(&self) -> String {
        let keys: Vec<String> = self.keys.iter().map(|k| format!("{k}=")).collect();
        let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
        one_of(&keys)
    }
}

/// The words of one filter, sorted out but not yet read.
struct Words<'a> {
    plain: Option<&'a str>,
    /// (key, value, whole word)
    options: Vec<(&'a str, &'a str, &'a str)>,
}

impl<'a> Words<'a> {
    fn option(&self, key: &str) -> Option<(&'a str, &'a str)> {
        self.options
            .iter()
            .find(|(k, _, _)| *k == key)
            .map(|&(_, v, w)| (v, w))
    }

    /// The word a rule's key refers to.
    fn word_for(&self, key: &str) -> Option<&'a str> {
        if key.is_empty() {
            self.plain
        } else {
            self.option(key).map(|(_, w)| w)
        }
    }
}

impl FromStr for FilterSpec {
    type Err = FilterError;

    fn from_str(s: &str) -> Result<Self, FilterError> {
        let mut iter = s.split_whitespace();
        let Some(name) = iter.next() else {
            return Err(FilterError::plain(
                "empty filter; expected a name such as \"gain -3dB\"",
            ));
        };
        let Some(shape) = SHAPES.iter().find(|sh| sh.name == name) else {
            let names: Vec<&str> = SHAPES.iter().map(|sh| sh.name).collect();
            return Err(FilterError::at(
                name,
                format!("unknown filter; expected {}", one_of(&names)),
            ));
        };
        let mut words = Words {
            plain: None,
            options: Vec::new(),
        };
        for w in iter {
            match w.split_once('=') {
                Some(("", _)) => return Err(FilterError::at(w, "expected key=value")),
                Some((k, v)) => {
                    if words.option(k).is_some() {
                        return Err(FilterError::at(w, format!("{k} is given twice")));
                    }
                    if !shape.keys.contains(&k) {
                        return Err(FilterError::at(
                            w,
                            if shape.keys.is_empty() {
                                format!("{name} takes no options")
                            } else {
                                format!(
                                    "{name} has no option {k}; expected {}",
                                    shape.expected_keys()
                                )
                            },
                        ));
                    }
                    words.options.push((k, v, w));
                }
                None if shape.needs.is_none() => {
                    return Err(FilterError::at(
                        w,
                        format!(
                            "{name} takes no plain value; expected {}",
                            shape.expected_keys()
                        ),
                    ));
                }
                None if words.plain.is_some() => {
                    return Err(FilterError::at(
                        w,
                        format!("{name} takes one plain value; give the rest as key=value"),
                    ));
                }
                None if !words.options.is_empty() => {
                    return Err(FilterError::at(
                        w,
                        "the plain value comes right after the filter's name",
                    ));
                }
                None => words.plain = Some(w),
            }
        }
        if let Some(needs) = shape.needs
            && words.plain.is_none()
        {
            return Err(FilterError::plain(needs));
        }
        let spec = read(name, &words)?;
        spec.rules()
            .map_err(|e| match e.key.and_then(|k| words.word_for(k)) {
                Some(w) => FilterError::at(w, e.message),
                None => FilterError::plain(e.message),
            })?;
        Ok(spec)
    }
}

/// Reads each word's value; ranges and relations are `rules`' business.
fn read(name: &str, w: &Words<'_>) -> Result<FilterSpec, FilterError> {
    let plain = w.plain.unwrap_or("");
    let value = |key: &str, expected: &str, f: fn(&str) -> Option<f64>| {
        w.option(key)
            .map(|(v, word)| f(v).ok_or_else(|| FilterError::at(word, expected)))
            .transpose()
    };
    Ok(match name {
        "gain" => FilterSpec::Gain {
            db: parse_db(plain)?,
        },
        "trim" => FilterSpec::Trim {
            start_seconds: value("start", words::TIME, words::time)?,
            end_seconds: value("end", words::TIME, words::time)?,
            duration_seconds: value("duration", words::TIME, words::time)?,
        },
        "fade" => FilterSpec::Fade {
            in_seconds: value("in", words::TIME, words::time)?,
            out_seconds: value("out", words::TIME, words::time)?,
            curve: named(w, "curve", "a curve", &Curve::NAMES)?
                .and_then(Curve::from_name)
                .unwrap_or_default(),
        },
        "normalize" => FilterSpec::Normalize {
            lufs: parse_lufs(plain)?,
            true_peak_dbtp: match w.option("peak") {
                Some((v, word)) => parse_dbtp(v).map_err(|_| FilterError::at(word, words::DBTP))?,
                None => DEFAULT_TRUE_PEAK_DBTP,
            },
        },
        "resample" => FilterSpec::Resample {
            rate: words::rate(plain).ok_or_else(|| FilterError::at(plain, words::RATE))?,
            quality: named(w, "quality", "a quality", &Quality::NAMES)?
                .and_then(Quality::from_name)
                .unwrap_or_default(),
        },
        _ => FilterSpec::Mix {
            channels: words::channels(plain)
                .ok_or_else(|| FilterError::at(plain, words::channels_expected()))?,
            matrix: match w.option("matrix") {
                Some((v, word)) => Some(words::matrix(v).ok_or_else(|| {
                    FilterError::at(word, "expected rows of numbers such as 0.5,0.5/1,0")
                })?),
                None => None,
            },
        },
    })
}

/// An option that must be one of `names`.
fn named<'a>(
    w: &Words<'a>,
    key: &str,
    expected: &str,
    names: &[&str],
) -> Result<Option<&'a str>, FilterError> {
    match w.option(key) {
        Some((v, _)) if names.contains(&v) => Ok(Some(v)),
        Some((_, word)) => Err(FilterError::at(
            word,
            format!("expected {expected}: {}", one_of(names)),
        )),
        None => Ok(None),
    }
}
