//! What the Adjust section holds: the text a person typed for each field, and the filters
//! it means. Values are never parsed here: the text gets its missing unit and goes to the
//! graph crate's parsers, then each filter through `FilterSpec::check`, so the sheet refuses
//! exactly what the command line would.

use crossfade_graph::filter::{
    Curve, DEFAULT_TRUE_PEAK_DBTP, FilterError, FilterSpec, parse_db, parse_lufs, parse_time,
};

/// What the Level row does to the loudness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LevelMode {
    #[default]
    Off,
    Gain,
    Normalize,
}

/// The rows a problem can sit under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdjustRow {
    Level,
    Trim,
    Fade,
}

/// The text fields. `Level` is the value of the current `LevelMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdjustField {
    Level,
    Start,
    End,
    FadeIn,
    FadeOut,
}

impl AdjustField {
    pub fn row(self) -> AdjustRow {
        match self {
            Self::Level => AdjustRow::Level,
            Self::Start | Self::End => AdjustRow::Trim,
            Self::FadeIn | Self::FadeOut => AdjustRow::Fade,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct Adjust {
    pub mode: LevelMode,
    // Each mode keeps its own value: -3 as gain is not -3 as a loudness.
    gain: String,
    normalize: String,
    start: String,
    end: String,
    fade_in: String,
    fade_out: String,
}

/// What the fields come to.
#[derive(Debug, Default)]
pub(super) struct Resolved {
    /// In the order the CLI's shortcut flags give: trim, fade, then gain or normalise.
    pub filters: Vec<FilterSpec>,
    pub problems: Vec<(AdjustRow, String)>,
    /// One phrase per row that is set, level first.
    pub summary: Vec<String>,
}

fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| if c == '\u{2212}' { '-' } else { c })
        .collect()
}

/// `-3`, `-3 dB` and `-3db` as the one spelling the parsers take, `-3dB`.
fn with_unit(text: &str, unit: &str) -> String {
    let s = clean(text);
    let at = s.len().saturating_sub(unit.len());
    let body = match s.get(at..) {
        Some(tail) if tail.eq_ignore_ascii_case(unit) => &s[..at],
        _ => &s,
    };
    format!("{body}{unit}")
}

/// A bare number is seconds; `1:30`, `90s` and `500ms` already say what they are.
fn time_word(text: &str) -> String {
    let s = clean(text);
    if s.contains(':') || s.ends_with('s') {
        s
    } else {
        format!("{s}s")
    }
}

/// The engine's refusal as a sentence, without the quoted word it names.
fn sentence(error: &FilterError) -> String {
    let text = error.to_string();
    let body = text
        .strip_prefix('"')
        .and_then(|rest| rest.split_once("\": "))
        .map_or(text.as_str(), |(_, message)| message);
    let mut chars = body.chars();
    let mut out: String = chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default();
    if !out.ends_with('.') {
        out.push('.');
    }
    out
}

/// `1:30`, `0:10`, `1:01:40`, `1:30.5`.
pub(super) fn clock(seconds: f64) -> String {
    let ms = (seconds * 1000.0).round() as u64;
    let (h, m, s, frac) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000);
    let mut out = if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{}:{s:02}", m)
    };
    if frac > 0 {
        out.push_str(format!(".{frac:03}").trim_end_matches('0'));
    }
    out
}

fn signed(x: f64) -> String {
    let text = x.to_string();
    match text.strip_prefix('-') {
        Some(rest) => format!("\u{2212}{rest}"),
        None => text,
    }
}

impl Adjust {
    fn slot(&mut self, field: AdjustField) -> &mut String {
        match (field, self.mode) {
            (AdjustField::Level, LevelMode::Normalize) => &mut self.normalize,
            (AdjustField::Level, _) => &mut self.gain,
            (AdjustField::Start, _) => &mut self.start,
            (AdjustField::End, _) => &mut self.end,
            (AdjustField::FadeIn, _) => &mut self.fade_in,
            (AdjustField::FadeOut, _) => &mut self.fade_out,
        }
    }

    pub(super) fn text(&self, field: AdjustField) -> &str {
        match (field, self.mode) {
            (AdjustField::Level, LevelMode::Normalize) => &self.normalize,
            (AdjustField::Level, _) => &self.gain,
            (AdjustField::Start, _) => &self.start,
            (AdjustField::End, _) => &self.end,
            (AdjustField::FadeIn, _) => &self.fade_in,
            (AdjustField::FadeOut, _) => &self.fade_out,
        }
    }

    pub(super) fn set_text(&mut self, field: AdjustField, text: String) {
        *self.slot(field) = text;
    }

    /// The strict spelling the engine's parser is given for a field that has text.
    pub(super) fn value(&self, field: AdjustField) -> Option<String> {
        let text = self.text(field);
        if text.trim().is_empty() {
            return None;
        }
        Some(match (field, self.mode) {
            (AdjustField::Level, LevelMode::Normalize) => with_unit(text, "LUFS"),
            (AdjustField::Level, _) => with_unit(text, "dB"),
            _ => time_word(text),
        })
    }

    /// `Ok(None)` for a field left empty.
    fn number(&self, field: AdjustField) -> Result<Option<f64>, String> {
        let Some(word) = self.value(field) else {
            return Ok(None);
        };
        let parsed = match (field, self.mode) {
            (AdjustField::Level, LevelMode::Normalize) => parse_lufs(&word),
            (AdjustField::Level, _) => parse_db(&word),
            _ => parse_time(&word),
        };
        parsed.map(Some).map_err(|_| {
            match (field, self.mode) {
                (AdjustField::Level, LevelMode::Normalize) => {
                    "The loudness target must be in LUFS, such as -16."
                }
                (AdjustField::Level, _) => "Gain must be a level in dB, such as -3.",
                (AdjustField::Start, _) => "Start must be a time such as 90, 1:30 or 500ms.",
                (AdjustField::End, _) => "End must be a time such as 90, 1:30 or 500ms.",
                (AdjustField::FadeIn, _) => "Fade in must be a time such as 2 or 500ms.",
                (AdjustField::FadeOut, _) => "Fade out must be a time such as 3 or 500ms.",
            }
            .to_owned()
        })
    }

    /// The first problem among a row's fields, else the values: `None` for a row left empty.
    fn row<const N: usize>(
        &self,
        row: AdjustRow,
        fields: [AdjustField; N],
        problems: &mut Vec<(AdjustRow, String)>,
    ) -> Option<[Option<f64>; N]> {
        let mut values = [None; N];
        for (value, field) in values.iter_mut().zip(fields) {
            match self.number(field) {
                Ok(v) => *value = v,
                Err(message) => {
                    problems.push((row, message));
                    return None;
                }
            }
        }
        values.iter().any(Option::is_some).then_some(values)
    }

    pub(super) fn resolve(&self) -> Resolved {
        let mut out = Resolved::default();
        // (filter, summary) for trim, fade and level, then put in their two orders.
        let mut parts: [Option<(FilterSpec, String)>; 3] = [None, None, None];

        if let Some([start, end]) = self.row(
            AdjustRow::Trim,
            [AdjustField::Start, AdjustField::End],
            &mut out.problems,
        ) {
            let text = match (start, end) {
                (Some(s), Some(e)) => format!("Trim {}\u{2013}{}", clock(s), clock(e)),
                (Some(s), None) => format!("Trim from {}", clock(s)),
                (None, e) => format!("Trim to {}", clock(e.unwrap_or_default())),
            };
            let spec = FilterSpec::Trim {
                start_seconds: start,
                end_seconds: end,
                duration_seconds: None,
            };
            parts[0] = Some((spec, text));
        }
        if let Some([fade_in, fade_out]) = self.row(
            AdjustRow::Fade,
            [AdjustField::FadeIn, AdjustField::FadeOut],
            &mut out.problems,
        ) {
            let text = match (fade_in, fade_out) {
                (Some(i), Some(o)) => format!("Fade in {i} s, out {o} s"),
                (Some(i), None) => format!("Fade in {i} s"),
                (None, o) => format!("Fade out {} s", o.unwrap_or_default()),
            };
            let spec = FilterSpec::Fade {
                in_seconds: fade_in,
                out_seconds: fade_out,
                curve: Curve::Linear,
            };
            parts[1] = Some((spec, text));
        }
        if self.mode != LevelMode::Off
            && let Some([Some(v)]) =
                self.row(AdjustRow::Level, [AdjustField::Level], &mut out.problems)
        {
            parts[2] = Some(match self.mode {
                LevelMode::Normalize => (
                    FilterSpec::Normalize {
                        lufs: v,
                        true_peak_dbtp: DEFAULT_TRUE_PEAK_DBTP,
                    },
                    format!("Normalize {} LUFS", signed(v)),
                ),
                _ => (
                    FilterSpec::Gain { db: v },
                    format!("Gain {}{} dB", if v > 0.0 { "+" } else { "" }, signed(v)),
                ),
            });
        }

        // A filter the engine's own rules refuse is a problem under its row, not a filter.
        let rows = [AdjustRow::Trim, AdjustRow::Fade, AdjustRow::Level];
        for (part, row) in parts.iter_mut().zip(rows) {
            if let Some((spec, _)) = part
                && let Err(e) = spec.check()
            {
                out.problems.push((row, sentence(&e)));
                *part = None;
            }
        }
        let [trim, fade, level] = parts;
        out.summary
            .extend(level.as_ref().map(|(_, text)| text.clone()));
        out.summary
            .extend(trim.as_ref().map(|(_, text)| text.clone()));
        out.summary
            .extend(fade.as_ref().map(|(_, text)| text.clone()));
        out.filters = [trim, fade, level]
            .into_iter()
            .flatten()
            .map(|(spec, _)| spec)
            .collect();
        out
    }
}
