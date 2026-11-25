//! The convert sheet: what the user has chosen, and the `Request` it makes.
//! Choices map one to one onto CLI arguments (see the parity suite). Whether a choice is
//! allowed is the engine's call (`crossfade_graph::validate`), so the sheet never offers
//! what a job would refuse.

use crate::command::Request;
use crate::inputs::InputEntry;
use crossfade_core::{CodecId, Error, SampleFormat, StreamInfo};
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
    /// Apple Lossless in MP4, `.m4a`.
    Mp4,
    /// Opus in Ogg, `.opus`.
    Opus,
}

impl Container {
    pub const ALL: [Container; 6] = [
        Container::Wav,
        Container::Flac,
        Container::Aiff,
        Container::OggFlac,
        Container::Mp4,
        Container::Opus,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Wav => "WAV",
            Self::Flac => "FLAC",
            Self::Aiff => "AIFF",
            Self::OggFlac => "Ogg FLAC",
            Self::Mp4 => "Apple Lossless",
            Self::Opus => "Opus",
        }
    }

    /// The extension the sheet writes.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Flac => "flac",
            Self::Aiff => "aiff",
            Self::OggFlac => "oga",
            Self::Mp4 => "m4a",
            Self::Opus => "opus",
        }
    }

    /// FLAC's compression level applies to both containers that hold FLAC.
    fn has_level(self) -> bool {
        matches!(self, Self::Flac | Self::OggFlac)
    }

    /// Opus is the one container with a bitrate.
    fn has_bitrate(self) -> bool {
        self == Self::Opus
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
            "m4a" | "mp4" | "m4b" => Some(Self::Mp4),
            "opus" => Some(Self::Opus),
            "aiff" | "aif" | "aifc" => Some(Self::Aiff),
            _ => None,
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
    preset(
        "alac",
        "Apple Lossless",
        Container::Mp4,
        FormatChoice::SameAsSource,
    ),
    preset("opus", "Opus", Container::Opus, FormatChoice::SameAsSource),
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
            Self::Extension => {
                let all: Vec<String> = writable_extensions()
                    .iter()
                    .map(|e| format!(".{e}"))
                    .collect();
                let (last, rest) = all.split_last().expect("some extension is writable");
                format!("Crossfade can save {} and {last} files.", rest.join(", "))
            }
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

/// An engine note as the sheet shows it: codecs by the names the sheet uses and the command
/// line's `--sample-format` as the Format row. A note it does not know stays as written.
pub fn plain_note(note: &str) -> String {
    const NAMES: [(&str, &str); 6] = [
        ("vorbis", "Vorbis"),
        ("opus", "Opus"),
        ("alac", "Apple Lossless"),
        ("flac", "FLAC"),
        ("mp3", "MP3"),
        ("aac", "AAC"),
    ];
    let mut text = note.replacen(
        "; choose --sample-format to change",
        "; choose a Format to change it",
        1,
    );
    for (codec, name) in NAMES {
        // Where the engine puts a codec's name: first, after a colon, or as an owner.
        if let Some(rest) = text.strip_prefix(&format!("{codec} ")) {
            text = format!("{name} {rest}");
        }
        text = text
            .replace(&format!(": {codec} "), &format!(": {name} "))
            .replace(&format!(": {codec}'s"), &format!(": {name}'s"))
            .replace(&format!(" as {codec}'s"), &format!(" as {name}'s"));
    }
    text
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
    bitrate: Option<u32>,
    copy: bool,
    overwrite: bool,
    sample_rate: Option<u32>,
    channels: Option<Channels>,
    adjust: Adjust,
    adjust_open: bool,
}

/// The one rate Opus is written at.
const OPUS_RATE: u32 = 48_000;

/// The bitrates the sheet offers for Opus besides the default, in bits per second.
const BITRATES: [u32; 7] = [32_000, 64_000, 96_000, 128_000, 160_000, 192_000, 256_000];

/// The same for three channels and more, whose whole stream is bigger.
const SURROUND_BITRATES: [u32; 7] = [
    128_000, 192_000, 256_000, 320_000, 384_000, 448_000, 512_000,
];

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
            bitrate: None,
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
        if self.container == Container::Opus {
            return FormatChoice::SameAsSource;
        }
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
            Container::Mp4 => format!(
                "Apple Lossless stores whole-number samples, so this will be saved as {bits}-bit."
            ),
            _ => format!(
                "This file type cannot store that format, so this will be saved as {bits}-bit."
            ),
        })
    }

    pub fn container(&self) -> Container {
        self.container
    }

    /// What `container` is called on this sheet: Ogg is FLAC's unless the stream copy carries
    /// something else (an Opus stream) into it.
    pub fn container_label(&self, container: Container) -> &'static str {
        let copies_other = self.copy()
            && self
                .stream
                .as_ref()
                .is_some_and(|s| s.codec != CodecId::Flac);
        if container == Container::OggFlac && copies_other {
            return "Ogg";
        }
        container.label()
    }

    /// Opus has no sample format to choose: it always codes from float.
    pub fn format_visible(&self) -> bool {
        self.container != Container::Opus || self.copy()
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
        if !container.has_bitrate() {
            self.bitrate = None;
        }
        if container == Container::Opus && self.sample_rate.is_some_and(|r| r != OPUS_RATE) {
            self.sample_rate = None;
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

    /// Opus's bitrate in bits per second; `None` is the codec's default for the channel count.
    pub fn bitrate(&self) -> Option<u32> {
        self.bitrate
    }

    pub fn bitrate_visible(&self) -> bool {
        self.container.has_bitrate() && !self.copy()
    }

    pub fn bitrate_enabled(&self) -> bool {
        self.bitrate_visible() && self.can_save()
    }

    pub fn set_bitrate(&mut self, bitrate: Option<u32>) {
        if self.container.has_bitrate() {
            self.bitrate = bitrate;
        }
    }

    /// The default plus the offered bitrates.
    pub fn bitrate_options(&self) -> Vec<Option<u32>> {
        let offered = if self.output_channels() > 2 {
            SURROUND_BITRATES
        } else {
            BITRATES
        };
        std::iter::once(None).chain(offered.map(Some)).collect()
    }

    /// Channels the output will have, as far as Opus's default bitrate cares.
    fn output_channels(&self) -> u16 {
        let own = self.stream.as_ref().map(|s| s.audio.layout().clone());
        let channels = match (&self.channels, &own) {
            (Some(c), Some(own)) => c.resolve(own).channels(),
            (Some(Channels::Layout(l)), None) => l.channels(),
            (Some(Channels::Count(n)), None) => *n,
            (None, Some(own)) => own.channels(),
            (None, None) => 2,
        };
        channels.clamp(1, 8)
    }

    /// `128 kb/s`; the default says what it is for this source.
    pub fn bitrate_label(&self, bitrate: Option<u32>) -> String {
        let kbps = |b: u32| format!("{} kb/s", b / 1000);
        match bitrate {
            Some(b) => kbps(b),
            None => format!(
                "Default ({})",
                kbps(crossfade_graph::default_opus_bitrate(
                    self.output_channels() as u8
                ))
            ),
        }
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
        // What was asked for counts as well as what will be written: a preset whose format the
        // container had to change (float into Apple Lossless) is still the one chosen, and the
        // hint says what changed. The list's order settles a tie.
        let format = self.format();
        PRESETS.iter().find(|p| {
            p.container == self.container
                && (p.format == self.requested || p.format == format)
                && p.level == self.level
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
                bitrate: self.bitrate.filter(|_| self.container.has_bitrate()),
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

    /// Opus is always coded at 48 kHz, so "Same as source" would say something untrue.
    fn fixed_rate(&self) -> bool {
        self.container == Container::Opus && !self.copy()
    }

    /// The rates to choose from: the source's own is what "Same as source" means. Opus has
    /// the one.
    pub fn rate_options(&self) -> Vec<Option<u32>> {
        if self.fixed_rate() {
            return vec![Some(OPUS_RATE)];
        }
        let own = self.stream.as_ref().map(|s| s.audio.sample_rate());
        std::iter::once(None)
            .chain(RATES.iter().copied().filter(|r| Some(*r) != own).map(Some))
            .collect()
    }

    /// The rate the row shows. The request only names one when the person chose it.
    pub fn sample_rate(&self) -> Option<u32> {
        self.sample_rate.or(self.fixed_rate().then_some(OPUS_RATE))
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
        self.adjust.set_mode(mode);
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

    /// What the engine says the job writes, when its values are good: the plan the command line
    /// prints the notes of.
    fn plan(&self) -> Option<crossfade_graph::OutputPlan> {
        let stream = self.stream.as_ref()?;
        if self.copy() || !self.adjust_problems().is_empty() {
            return None;
        }
        describe(
            stream,
            &format!("x.{}", self.container.extension()),
            &self.audio(),
        )
        .ok()
    }

    /// The plan, when processing changes what is written (asked for, or a rate the container
    /// forces).
    fn processed_plan(&self) -> Option<crossfade_graph::OutputPlan> {
        let audio = self.audio();
        let plan = self.plan()?;
        let own_rate = self.stream.as_ref()?.audio.sample_rate();
        let processed = audio.sample_rate.is_some()
            || audio.channels.is_some()
            || !audio.filters.is_empty()
            || plan.output.audio.sample_rate() != own_rate;
        processed.then_some(plan)
    }

    /// The assumptions the plan makes, such as "treating 6 unlabelled channels as 5.1" or "mp3
    /// has no bit depth: writing 16-bit": the notes the command line prints, in the sheet's words.
    pub fn notes(&self) -> Vec<String> {
        self.engine_notes().iter().map(|n| plain_note(n)).collect()
    }

    /// The plan's notes exactly as the engine wrote them.
    pub fn engine_notes(&self) -> Vec<String> {
        self.plan().map(|p| p.notes).unwrap_or_default()
    }

    /// What will be written, when processing changes it: `48 kHz · mono · 16-bit · 1:20.000`.
    pub fn processing_hint(&self) -> Option<String> {
        let plan = self.processed_plan()?;
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
        let mut parts = vec![Self::rate_label(Some(rate)), layout];
        // A lossy codec has no depth to state.
        if !out.codec.is_lossy() {
            parts.push(depth);
        }
        if let Some(frames) = out.frames {
            parts.push(clock_exact(frames as f64 / f64::from(rate)));
        }
        Some(parts.join(" \u{b7} "))
    }

    /// `problem`, then the Channels choices that plan cleanly with it, when there are any.
    fn with_channel_advice(&self, problem: String) -> String {
        let ways: Vec<String> = self
            .channel_options()
            .into_iter()
            .flatten()
            .filter(|c| {
                let audio = AudioSettings {
                    channels: Some(c.clone()),
                    ..self.audio()
                };
                self.check(self.container, &audio).is_ok()
            })
            .map(|c| Self::channel_label(&Some(c)))
            .collect();
        if ways.is_empty() {
            return problem;
        }
        format!("{problem} Choose {} under Channels.", ways.join(" or "))
    }

    fn engine_problem(&self) -> Option<Problem> {
        let error = self.check(self.container, &self.audio()).err()?;
        let what = match error {
            Error::Unsupported { what } => what,
            other => other.to_string(),
        };
        let stream = self.stream.as_ref()?;
        let channels = stream.audio.layout().channels();
        let text = if what.starts_with("opus stores") || what.starts_with("opus holds at most") {
            let problem = what.split("; ").next().unwrap_or(&what);
            self.with_channel_advice(plain(problem))
        } else if what.contains("at most 8 channels") {
            let name = if self.container == Container::Mp4 {
                "Apple Lossless"
            } else {
                "FLAC"
            };
            format!("{name} can store up to 8 channels; this file has {channels}.")
        } else if what.starts_with("alac stores") {
            // The rest is advice about a matrix, which the sheet has no way to give.
            let problem = what.split("; ").next().unwrap_or(&what);
            self.with_channel_advice(plain(&problem.replacen("alac", "Apple Lossless", 1)))
        } else {
            plain(&what.replacen("alac", "Apple Lossless", 1))
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
