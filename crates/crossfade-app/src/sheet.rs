//! The convert sheet: what the user has chosen, and the `Request` it makes.
//! Choices map one to one onto CLI arguments (see the parity suite). Whether a choice is
//! allowed is the engine's call (`crossfade_graph::validate`), so the sheet never offers
//! what a job would refuse.

use crate::command::Request;
use crate::inputs::InputEntry;
use crossfade_core::{Error, SampleFormat, StreamInfo};
use crossfade_graph::filter::{Channels, FilterSpec, parse_channels};
use crossfade_graph::{
    AudioSettings, Job, JobInput, JobOutput, describe, validate, writable_extensions,
};
use std::path::Path;

mod adjust;

use adjust::{Adjust, clock_exact};
pub use adjust::{AdjustField, AdjustRow, LevelMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatChoice {
    SameAsSource,
    Format(SampleFormat),
}

/// The containers the sheet can write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    Wav,
    Flac,
    Aiff,
    /// FLAC in an Ogg stream, `.oga`.
    OggFlac,
}

impl Container {
    pub const ALL: [Container; 4] = [
        Container::Wav,
        Container::Flac,
        Container::Aiff,
        Container::OggFlac,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Wav => "WAV",
            Self::Flac => "FLAC",
            Self::Aiff => "AIFF",
            Self::OggFlac => "Ogg FLAC",
        }
    }

    /// The extension the sheet writes.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Flac => "flac",
            Self::Aiff => "aiff",
            Self::OggFlac => "oga",
        }
    }

    /// FLAC's compression level applies to both containers that hold FLAC.
    fn has_level(self) -> bool {
        matches!(self, Self::Flac | Self::OggFlac)
    }

    /// The container an extension names, if the engine can write it.
    fn from_extension(ext: &str) -> Option<Self> {
        let ext = ext.to_ascii_lowercase();
        if !writable_extensions().contains(&ext.as_str()) {
            return None;
        }
        match ext.as_str() {
            "wav" => Some(Self::Wav),
            "flac" => Some(Self::Flac),
            "oga" | "ogg" => Some(Self::OggFlac),
            _ => Some(Self::Aiff),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    pub container: Container,
    pub format: FormatChoice,
    pub level: Option<u8>,
}

const fn preset(
    id: &'static str,
    label: &'static str,
    container: Container,
    format: FormatChoice,
) -> Preset {
    Preset {
        id,
        label,
        container,
        format,
        level: None,
    }
}

pub const PRESETS: &[Preset] = &[
    preset(
        "same",
        "Same as source",
        Container::Wav,
        FormatChoice::SameAsSource,
    ),
    preset(
        "cd",
        "16-bit (CD)",
        Container::Wav,
        FormatChoice::Format(SampleFormat::S16),
    ),
    preset(
        "studio",
        "24-bit",
        Container::Wav,
        FormatChoice::Format(SampleFormat::S24),
    ),
    preset(
        "float",
        "32-bit float",
        Container::Wav,
        FormatChoice::Format(SampleFormat::F32),
    ),
    preset("flac", "FLAC", Container::Flac, FormatChoice::SameAsSource),
    preset(
        "flac-cd",
        "FLAC 16-bit",
        Container::Flac,
        FormatChoice::Format(SampleFormat::S16),
    ),
    preset(
        "flac-24",
        "FLAC 24-bit",
        Container::Flac,
        FormatChoice::Format(SampleFormat::S24),
    ),
    preset("aiff", "AIFF", Container::Aiff, FormatChoice::SameAsSource),
];

/// FLAC's highest level.
const MAX_LEVEL: u8 = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    NoOutput,
    OutputIsInput,
    OutputExists,
    /// The path's extension is not one we can save.
    Extension,
    /// The engine would refuse this source with these choices, in plain words.
    Source(String),
    /// A value in the Adjust section that does not parse or that the filter rules refuse.
    Adjust {
        row: AdjustRow,
        message: String,
    },
}

impl Problem {
    pub fn message(&self) -> String {
        match self {
            Self::NoOutput => "Choose where to save the converted file.".into(),
            Self::OutputIsInput => "The output would replace the input file.".into(),
            Self::OutputExists => {
                "That file already exists. Turn on replace to overwrite it.".into()
            }
            Self::Extension => "Crossfade can save .wav, .flac, .oga, .ogg and .aiff files.".into(),
            Self::Source(text) | Self::Adjust { message: text, .. } => text.clone(),
        }
    }
}

/// An engine refusal as a sentence for a person: command-line flags and the advice that
/// names them are dropped, since the sheet has no flags.
pub fn plain(what: &str) -> String {
    let kept: Vec<&str> = what.split("; ").filter(|p| !p.contains("--")).collect();
    if kept.is_empty() {
        return "Crossfade cannot save this combination of choices.".into();
    }
    let text = kept.join("; ");
    let mut chars = text.chars();
    let mut out: String = chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default();
    if !out.ends_with('.') {
        out.push('.');
    }
    out
}

#[derive(Debug, Clone)]
pub struct ConvertSheet {
    input: String,
    stream: Option<StreamInfo>,
    output: String,
    // Once the user has typed a path, only the extension follows the container.
    output_edited: bool,
    container: Container,
    // What the user asked for; `format()` is what the container can hold of it.
    requested: FormatChoice,
    level: Option<u8>,
    copy: bool,
    overwrite: bool,
    sample_rate: Option<u32>,
    channels: Option<Channels>,
    adjust: Adjust,
    adjust_open: bool,
}

/// The output rates the sheet offers besides the source's own.
const RATES: [u32; 6] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000];

impl ConvertSheet {
    pub fn for_input(entry: &InputEntry) -> Self {
        let mut sheet = Self {
            input: entry.path.clone(),
            stream: entry.stream().cloned(),
            output: String::new(),
            output_edited: false,
            container: Container::Wav,
            requested: FormatChoice::SameAsSource,
            level: None,
            copy: false,
            overwrite: false,
            sample_rate: None,
            channels: None,
            adjust: Adjust::default(),
            adjust_open: false,
        };
        sheet.output = sheet.default_output();
        sheet
    }

    fn input_container(&self) -> Option<Container> {
        Path::new(&self.input)
            .extension()
            .and_then(|e| Container::from_extension(&e.to_string_lossy()))
    }

    fn default_output(&self) -> String {
        let path = Path::new(&self.input);
        let stem = path
            .file_stem()
            .map_or_else(|| "output".to_owned(), |s| s.to_string_lossy().into_owned());
        let ext = self.container.extension();
        let explicit = match self.format() {
            FormatChoice::Format(f) if !self.copy() => Some(f),
            _ => None,
        };
        let name = match explicit {
            Some(f) => format!("{stem}-{}.{ext}", f.name()),
            // A new container is a new name; the same one must not name the input.
            None if self.input_container().is_some_and(|c| c != self.container) => {
                format!("{stem}.{ext}")
            }
            None => format!("{stem}-copy.{ext}"),
        };
        path.with_file_name(name).to_string_lossy().into_owned()
    }

    /// Whether the engine takes `settings` for this source in `container`. Without a probed
    /// stream there is nothing to ask, so everything passes.
    fn check(&self, container: Container, settings: &AudioSettings) -> Result<(), Error> {
        let Some(stream) = &self.stream else {
            return Ok(());
        };
        validate(stream, &format!("x.{}", container.extension()), settings).map(|_| ())
    }

    fn format_settings(choice: FormatChoice, level: Option<u8>) -> AudioSettings {
        AudioSettings {
            sample_format: match choice {
                FormatChoice::SameAsSource => None,
                FormatChoice::Format(f) => Some(f),
            },
            codec: None,
            compression_level: level,
            ..AudioSettings::default()
        }
    }

    fn copy_settings() -> AudioSettings {
        AudioSettings {
            codec: Some("copy".into()),
            ..AudioSettings::default()
        }
    }

    fn accepts(&self, container: Container, choice: FormatChoice) -> bool {
        self.check(container, &Self::format_settings(choice, None))
            .is_ok()
    }

    /// The choices the engine accepts for this source and the current container.
    pub fn format_options(&self) -> Vec<FormatChoice> {
        use SampleFormat::*;
        std::iter::once(FormatChoice::SameAsSource)
            .chain([U8, S16, S24, S32, F32, F64].map(FormatChoice::Format))
            .filter(|choice| self.accepts(self.container, *choice))
            .collect()
    }

    /// What will be written: the requested format, or the closest one the container holds.
    pub fn format(&self) -> FormatChoice {
        if self.accepts(self.container, self.requested) {
            return self.requested;
        }
        [SampleFormat::S24, SampleFormat::S16, SampleFormat::U8]
            .map(FormatChoice::Format)
            .into_iter()
            .find(|choice| self.accepts(self.container, *choice))
            .unwrap_or(self.requested)
    }

    /// One line saying why the saved format differs from the one asked for.
    pub fn hint(&self) -> Option<String> {
        let format = self.format();
        if self.copy() || format == self.requested {
            return None;
        }
        let FormatChoice::Format(f) = format else {
            return None;
        };
        let bits = f.bits();
        Some(match self.container {
            Container::Flac | Container::OggFlac => {
                format!("FLAC stores whole-number samples, so this will be saved as {bits}-bit.")
            }
            _ => format!(
                "This file type cannot store that format, so this will be saved as {bits}-bit."
            ),
        })
    }

    pub fn container(&self) -> Container {
        self.container
    }

    pub fn set_container(&mut self, container: Container) {
        self.adopt(container);
        self.refresh_output();
    }

    /// Switch container and drop what only the old one meant.
    fn adopt(&mut self, container: Container) {
        self.container = container;
        if !container.has_level() {
            self.level = None;
        }
        if !self.copy_available() {
            self.copy = false;
        }
    }

    fn refresh_output(&mut self) {
        if !self.output_edited {
            self.output = self.default_output();
            return;
        }
        let current = Path::new(&self.output)
            .extension()
            .and_then(|e| Container::from_extension(&e.to_string_lossy()));
        if !self.output.trim().is_empty() && current != Some(self.container) {
            self.output = Path::new(&self.output)
                .with_extension(self.container.extension())
                .to_string_lossy()
                .into_owned();
        }
    }

    pub fn apply_preset(&mut self, id: &str) {
        if let Some(preset) = PRESETS.iter().find(|p| p.id == id) {
            self.requested = preset.format;
            self.copy = false;
            self.adopt(preset.container);
            self.level = preset.level.filter(|_| preset.container.has_level());
            self.refresh_output();
        }
    }

    pub fn set_format(&mut self, format: FormatChoice) {
        self.requested = format;
        self.refresh_output();
    }

    /// FLAC's level, 0 to 8; `None` is the encoder's default.
    pub fn level(&self) -> Option<u8> {
        self.level
    }

    pub fn level_visible(&self) -> bool {
        self.container.has_level()
    }

    pub fn level_enabled(&self) -> bool {
        self.level_visible() && !self.copy() && self.can_save()
    }

    /// Whether the source can go into this container at all, as audio or as a copy.
    fn can_save(&self) -> bool {
        self.copy_available() || !self.format_options().is_empty()
    }

    pub fn set_level(&mut self, level: Option<u8>) {
        if self.level_visible() && level.is_none_or(|l| l <= MAX_LEVEL) {
            self.level = level;
        }
    }

    /// Whether the engine can pass the source's audio into this container untouched.
    pub fn copy_available(&self) -> bool {
        self.stream.is_some() && self.check(self.container, &Self::copy_settings()).is_ok()
    }

    pub fn copy(&self) -> bool {
        self.copy && self.copy_available()
    }

    pub fn set_copy(&mut self, on: bool) {
        self.copy = on && self.copy_available();
        self.refresh_output();
    }

    /// Copying leaves nothing to choose about the audio, and neither does a container
    /// that cannot hold the source.
    pub fn format_enabled(&self) -> bool {
        !self.copy() && self.can_save()
    }

    pub fn set_output(&mut self, path: String) {
        self.output = path;
        self.output_edited = true;
        let typed = Path::new(&self.output)
            .extension()
            .and_then(|e| Container::from_extension(&e.to_string_lossy()));
        if let Some(container) = typed.filter(|c| *c != self.container) {
            self.adopt(container);
        }
    }

    pub fn set_overwrite(&mut self, overwrite: bool) {
        self.overwrite = overwrite;
    }

    /// The preset these choices equal, if any.
    pub fn preset(&self) -> Option<&'static Preset> {
        if self.copy() {
            return None;
        }
        PRESETS.iter().find(|p| {
            p.container == self.container && p.format == self.format() && p.level == self.level
        })
    }

    pub fn output(&self) -> &str {
        &self.output
    }

    pub fn overwrite(&self) -> bool {
        self.overwrite
    }

    fn audio(&self) -> AudioSettings {
        if self.copy() {
            Self::copy_settings()
        } else {
            let level = self.level.filter(|_| self.container.has_level());
            AudioSettings {
                sample_rate: self.sample_rate,
                channels: self.channels.clone(),
                filters: self.adjust.resolve().filters,
                ..Self::format_settings(self.format(), level)
            }
        }
    }

    /// `kHz` as people write it: `44.1 kHz`, `48 kHz`.
    pub fn rate_label(rate: Option<u32>) -> String {
        let Some(hz) = rate else {
            return "Same as source".into();
        };
        let khz = format!("{:.3}", f64::from(hz) / 1000.0);
        format!("{} kHz", khz.trim_end_matches('0').trim_end_matches('.'))
    }

    /// The rates to choose from: the source's own is what "Same as source" means.
    pub fn rate_options(&self) -> Vec<Option<u32>> {
        let own = self.stream.as_ref().map(|s| s.audio.sample_rate());
        std::iter::once(None)
            .chain(RATES.into_iter().filter(|r| Some(*r) != own).map(Some))
            .collect()
    }

    pub fn sample_rate(&self) -> Option<u32> {
        self.sample_rate
    }

    pub fn set_sample_rate(&mut self, rate: Option<u32>) {
        self.sample_rate = rate;
    }

    pub fn channel_label(channels: &Option<Channels>) -> String {
        match channels {
            None => "Same as source".into(),
            Some(Channels::Layout(l)) => match l.name() {
                Some("mono") => "Mono".into(),
                Some("stereo") => "Stereo".into(),
                Some(name) => name.to_owned(),
                None => format!("{} channels", l.channels()),
            },
            Some(Channels::Count(n)) => format!("{n} channels"),
        }
    }

    /// Mono and stereo, less the source's own layout.
    pub fn channel_options(&self) -> Vec<Option<Channels>> {
        let own = self.stream.as_ref().map(|s| s.audio.layout().clone());
        std::iter::once(None)
            .chain(["mono", "stereo"].into_iter().filter_map(|name| {
                let choice = parse_channels(name).ok()?;
                let same = own.as_ref().is_some_and(|l| choice.resolve(l) == *l);
                (!same).then_some(Some(choice))
            }))
            .collect()
    }

    pub fn channels(&self) -> Option<&Channels> {
        self.channels.as_ref()
    }

    pub fn set_channels(&mut self, channels: Option<Channels>) {
        self.channels = channels;
    }

    pub fn adjust_open(&self) -> bool {
        self.adjust_open
    }

    pub fn set_adjust_open(&mut self, open: bool) {
        self.adjust_open = open;
    }

    pub fn level_mode(&self) -> LevelMode {
        self.adjust.mode
    }

    pub fn set_level_mode(&mut self, mode: LevelMode) {
        self.adjust.mode = mode;
    }

    /// What the field holds, as typed.
    pub fn adjust_text(&self, field: AdjustField) -> &str {
        self.adjust.text(field)
    }

    pub fn set_adjust_text(&mut self, field: AdjustField, text: String) {
        self.adjust.set_text(field, text);
    }

    /// The field's text in the one spelling the engine's parsers take (`-3dB`, `90s`);
    /// `None` when it is empty.
    pub fn adjust_value(&self, field: AdjustField) -> Option<String> {
        self.adjust.value(field)
    }

    /// Whether the controls for rate, channels and Adjust can be used: stream copy passes
    /// the packets through untouched.
    pub fn processing_enabled(&self) -> bool {
        !self.copy()
    }

    /// Why they cannot, in the engine's words.
    pub fn processing_disabled_reason(&self) -> Option<String> {
        if !self.copy() {
            return None;
        }
        let asks = AudioSettings {
            filters: vec![FilterSpec::Gain { db: 0.0 }],
            ..Self::copy_settings()
        };
        let what = match self.check(self.container, &asks) {
            Err(Error::Unsupported { what }) => what,
            _ => return Some("Stream copy leaves the audio untouched.".into()),
        };
        let text = what.replacen("--codec copy", "Stream copy", 1);
        let mut text = match text.split_once("; ") {
            Some((kept, _)) => format!("{kept})"),
            None => text,
        };
        text.push('.');
        Some(text)
    }

    /// The problem under a row of the Adjust section, if there is one.
    pub fn adjust_problem(&self, row: AdjustRow) -> Option<String> {
        self.adjust_problems()
            .into_iter()
            .find_map(|(r, message)| (r == row).then_some(message))
    }

    fn adjust_problems(&self) -> Vec<(AdjustRow, String)> {
        if self.copy() {
            return Vec::new();
        }
        self.adjust.resolve().problems
    }

    /// One muted line for the collapsed Adjust row: `Normalize −16 LUFS · Trim 0:10–1:30`.
    pub fn adjust_summary(&self) -> Option<String> {
        if self.copy() {
            return None;
        }
        let parts = self.adjust.resolve().summary;
        (!parts.is_empty()).then(|| parts.join(" \u{b7} "))
    }

    /// What will be written, when processing changes it: `48 kHz · mono · 16-bit · 1:20.000`.
    pub fn processing_hint(&self) -> Option<String> {
        let stream = self.stream.as_ref()?;
        let audio = self.audio();
        let processed =
            audio.sample_rate.is_some() || audio.channels.is_some() || !audio.filters.is_empty();
        if self.copy() || !processed || !self.adjust_problems().is_empty() {
            return None;
        }
        let plan = describe(stream, &format!("x.{}", self.container.extension()), &audio).ok()?;
        let out = &plan.output;
        let layout = out.audio.layout();
        let layout = layout
            .name()
            .map_or_else(|| format!("{} ch", layout.channels()), str::to_owned);
        let format = out.audio.format();
        let bits = out.bits_per_raw_sample.map_or(format.bits(), usize::from);
        let depth = if format.is_float() {
            format!("{bits}-bit float")
        } else {
            format!("{bits}-bit")
        };
        let rate = out.audio.sample_rate();
        let mut parts = vec![Self::rate_label(Some(rate)), layout, depth];
        if let Some(frames) = out.frames {
            parts.push(clock_exact(frames as f64 / f64::from(rate)));
        }
        Some(parts.join(" \u{b7} "))
    }

    fn engine_problem(&self) -> Option<Problem> {
        let error = self.check(self.container, &self.audio()).err()?;
        let what = match error {
            Error::Unsupported { what } => what,
            other => other.to_string(),
        };
        let stream = self.stream.as_ref()?;
        let channels = stream.audio.layout().channels();
        let text = if what.contains("at most 8 channels") {
            format!("FLAC can store up to 8 channels; this file has {channels}.")
        } else {
            plain(&what)
        };
        Some(Problem::Source(text))
    }

    pub fn problems(&self, exists: &dyn Fn(&str) -> bool) -> Vec<Problem> {
        if self.output.trim().is_empty() {
            return vec![Problem::NoOutput];
        }
        let mut problems = Vec::new();
        if Path::new(&self.output) == Path::new(&self.input) {
            problems.push(Problem::OutputIsInput);
        }
        if !self.overwrite && exists(&self.output) {
            problems.push(Problem::OutputExists);
        }
        let ext = Path::new(&self.output)
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if Container::from_extension(&ext).is_none() {
            problems.push(Problem::Extension);
        } else {
            problems.extend(
                self.adjust_problems()
                    .into_iter()
                    .map(|(row, message)| Problem::Adjust { row, message }),
            );
            problems.extend(self.engine_problem());
        }
        problems
    }

    pub fn build(&self, exists: &dyn Fn(&str) -> bool) -> Result<Request, Vec<Problem>> {
        let problems = self.problems(exists);
        if !problems.is_empty() {
            return Err(problems);
        }
        Ok(self.draft())
    }

    /// The request as the choices stand, without asking whether the file system or the
    /// engine will take it; [`build`](Self::build) is the checked one.
    pub fn draft(&self) -> Request {
        Request {
            job: Job {
                version: Job::VERSION,
                inputs: vec![JobInput {
                    path: self.input.clone(),
                }],
                outputs: vec![JobOutput {
                    path: self.output.clone(),
                    audio: self.audio(),
                }],
            },
            overwrite: self.overwrite,
        }
    }
}
