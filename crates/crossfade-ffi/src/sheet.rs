//! The convert sheet across the boundary: the app's `ConvertSheet`, its state as one JSON
//! document, and changes as JSON applied through its own setters, so it alone decides what
//! is offered and what can be queued.

use crate::error::Failure;
use crate::inspect;
use crossfade_app::Request;
use crossfade_app::command_line;
use crossfade_app::inputs::InputState;
use crossfade_app::inspector::sample_format_name;
use crossfade_app::sheet::{
    AdjustField, AdjustRow, Container, ConvertSheet, FormatChoice, LevelMode, PRESETS, Problem,
};
use crossfade_core::SampleFormat;
use crossfade_graph::Job;
use crossfade_graph::filter::{Channels, parse_channels};
use serde::Serialize;
use serde_json::{Map, Value};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

/// A sheet any thread may use; calls are serialised.
pub struct Sheet {
    sheet: Mutex<ConvertSheet>,
}

/// Whether a path is taken, as the desktop asks it.
pub(crate) fn exists(path: &str) -> bool {
    Path::new(path).exists()
}

fn container_id(c: Container) -> &'static str {
    c.extension()
}

fn container_from(id: &str) -> Option<Container> {
    Container::ALL.into_iter().find(|c| c.extension() == id)
}

fn format_id(f: FormatChoice) -> &'static str {
    match f {
        FormatChoice::SameAsSource => "same",
        FormatChoice::Format(f) => f.name(),
    }
}

fn format_label(f: FormatChoice) -> String {
    match f {
        FormatChoice::SameAsSource => "Same as source".into(),
        FormatChoice::Format(f) => sample_format_name(f.name()),
    }
}

fn mode_id(m: LevelMode) -> &'static str {
    match m {
        LevelMode::Off => "off",
        LevelMode::Gain => "gain",
        LevelMode::Normalize => "normalize",
    }
}

fn row_id(r: AdjustRow) -> &'static str {
    match r {
        AdjustRow::Level => "level",
        AdjustRow::Trim => "trim",
        AdjustRow::Fade => "fade",
    }
}

/// One setter call, checked before any is made.
enum Change {
    Preset(&'static str),
    Container(Container),
    Output(String),
    Copy(bool),
    Format(FormatChoice),
    Level(Option<u8>),
    Bitrate(Option<u32>),
    SampleRate(Option<u32>),
    Channels(Option<Channels>),
    Overwrite(bool),
    AdjustOpen(bool),
    LevelMode(LevelMode),
    Text(AdjustField, String),
}

/// The keys a change object may hold, in the order one object's keys are applied.
pub(crate) const CHANGE_KEYS: [&str; 17] = [
    "preset",
    "container",
    "output",
    "copy",
    "format",
    "compression_level",
    "bitrate",
    "sample_rate",
    "channels",
    "overwrite",
    "adjust_open",
    "level_mode",
    "level_value",
    "trim_start",
    "trim_end",
    "fade_in",
    "fade_out",
];

fn wrong(key: &str, wants: &str) -> Failure {
    Failure::json(format!("\"{key}\" takes {wants}"))
}

fn text(key: &str, v: &Value) -> Result<String, Failure> {
    v.as_str()
        .map(str::to_owned)
        .ok_or_else(|| wrong(key, "a string"))
}

fn flag(key: &str, v: &Value) -> Result<bool, Failure> {
    v.as_bool().ok_or_else(|| wrong(key, "true or false"))
}

fn whole<T: TryFrom<u64>>(key: &str, v: &Value) -> Result<Option<T>, Failure> {
    if v.is_null() {
        return Ok(None);
    }
    v.as_u64()
        .and_then(|n| T::try_from(n).ok())
        .map(Some)
        .ok_or_else(|| wrong(key, "a whole number or null"))
}

fn change(key: &str, v: &Value) -> Result<Change, Failure> {
    Ok(match key {
        "preset" => {
            let id = text(key, v)?;
            let preset = PRESETS
                .iter()
                .find(|p| p.id == id)
                .ok_or_else(|| wrong(key, "a preset id from the state's options"))?;
            Change::Preset(preset.id)
        }
        "container" => Change::Container(
            container_from(&text(key, v)?)
                .ok_or_else(|| wrong(key, "a container id from the state's options"))?,
        ),
        "output" => Change::Output(text(key, v)?),
        "copy" => Change::Copy(flag(key, v)?),
        "format" => {
            let id = text(key, v)?;
            Change::Format(if id == "same" {
                FormatChoice::SameAsSource
            } else {
                FormatChoice::Format(
                    SampleFormat::from_name(&id)
                        .ok_or_else(|| wrong(key, "\"same\" or a format id"))?,
                )
            })
        }
        "compression_level" => Change::Level(whole(key, v)?),
        "bitrate" => Change::Bitrate(whole(key, v)?),
        "sample_rate" => Change::SampleRate(whole(key, v)?),
        "channels" => Change::Channels(if v.is_null() {
            None
        } else {
            Some(
                parse_channels(&text(key, v)?)
                    .map_err(|_| wrong(key, "a layout name, a channel count or null"))?,
            )
        }),
        "overwrite" => Change::Overwrite(flag(key, v)?),
        "adjust_open" => Change::AdjustOpen(flag(key, v)?),
        "level_mode" => Change::LevelMode(match text(key, v)?.as_str() {
            "off" => LevelMode::Off,
            "gain" => LevelMode::Gain,
            "normalize" => LevelMode::Normalize,
            _ => return Err(wrong(key, "\"off\", \"gain\" or \"normalize\"")),
        }),
        "level_value" => Change::Text(AdjustField::Level, text(key, v)?),
        "trim_start" => Change::Text(AdjustField::Start, text(key, v)?),
        "trim_end" => Change::Text(AdjustField::End, text(key, v)?),
        "fade_in" => Change::Text(AdjustField::FadeIn, text(key, v)?),
        "fade_out" => Change::Text(AdjustField::FadeOut, text(key, v)?),
        other => return Err(Failure::json(format!("unknown change \"{other}\""))),
    })
}

fn object_changes(object: &Map<String, Value>, out: &mut Vec<Change>) -> Result<(), Failure> {
    if let Some(unknown) = object.keys().find(|k| !CHANGE_KEYS.contains(&k.as_str())) {
        return Err(Failure::json(format!("unknown change \"{unknown}\"")));
    }
    for key in CHANGE_KEYS {
        if let Some(v) = object.get(key) {
            out.push(change(key, v)?);
        }
    }
    Ok(())
}

/// Every change the text asks for, or the first reason it cannot be read.
fn parse_changes(json: &str) -> Result<Vec<Change>, Failure> {
    let value: Value =
        serde_json::from_str(json).map_err(|e| Failure::json(format!("not JSON: {e}")))?;
    let not_object = || Failure::json("changes are an object or a list of objects");
    let mut changes = Vec::new();
    match &value {
        Value::Object(o) => object_changes(o, &mut changes)?,
        Value::Array(items) => {
            for item in items {
                object_changes(item.as_object().ok_or_else(not_object)?, &mut changes)?;
            }
        }
        _ => return Err(not_object()),
    }
    Ok(changes)
}

fn apply(sheet: &mut ConvertSheet, change: Change) {
    match change {
        Change::Preset(id) => sheet.apply_preset(id),
        Change::Container(c) => sheet.set_container(c),
        Change::Output(path) => sheet.set_output(path),
        Change::Copy(on) => sheet.set_copy(on),
        Change::Format(f) => sheet.set_format(f),
        Change::Level(l) => sheet.set_level(l),
        Change::Bitrate(b) => sheet.set_bitrate(b),
        Change::SampleRate(r) => sheet.set_sample_rate(r),
        Change::Channels(c) => sheet.set_channels(c),
        Change::Overwrite(on) => sheet.set_overwrite(on),
        Change::AdjustOpen(on) => sheet.set_adjust_open(on),
        Change::LevelMode(m) => sheet.set_level_mode(m),
        Change::Text(field, text) => sheet.set_adjust_text(field, text),
    }
}

#[derive(Serialize)]
struct Id {
    id: &'static str,
    label: String,
}

#[derive(Serialize)]
struct Labelled<T> {
    value: T,
    label: String,
}

#[derive(Serialize)]
struct Presets {
    value: Option<&'static str>,
    options: Vec<Id>,
}

#[derive(Serialize)]
struct ContainerOption {
    id: &'static str,
    label: &'static str,
    extension: &'static str,
}

#[derive(Serialize)]
struct Containers {
    value: &'static str,
    label: &'static str,
    options: Vec<ContainerOption>,
}

#[derive(Serialize)]
struct Format {
    value: &'static str,
    label: String,
    visible: bool,
    enabled: bool,
    hint: Option<String>,
    options: Vec<Id>,
}

#[derive(Serialize)]
struct Level {
    value: Option<u8>,
    visible: bool,
    enabled: bool,
    options: Vec<Option<u8>>,
}

#[derive(Serialize)]
struct Bitrate {
    value: Option<u32>,
    label: String,
    visible: bool,
    enabled: bool,
    options: Vec<Labelled<Option<u32>>>,
}

#[derive(Serialize)]
struct Copy {
    value: bool,
    available: bool,
}

#[derive(Serialize)]
struct Rate {
    value: Option<u32>,
    label: String,
    enabled: bool,
    options: Vec<Labelled<Option<u32>>>,
}

#[derive(Serialize)]
struct ChannelsState {
    value: Option<String>,
    label: String,
    enabled: bool,
    options: Vec<Labelled<Option<String>>>,
}

#[derive(Serialize)]
struct RowProblems {
    level: Option<String>,
    trim: Option<String>,
    fade: Option<String>,
}

#[derive(Serialize)]
struct Adjust {
    open: bool,
    enabled: bool,
    summary: Option<String>,
    level_mode: &'static str,
    level_value: String,
    trim_start: String,
    trim_end: String,
    fade_in: String,
    fade_out: String,
    problems: RowProblems,
}

#[derive(Serialize)]
struct ProblemState {
    kind: &'static str,
    row: Option<&'static str>,
    message: String,
}

#[derive(Serialize)]
pub(crate) struct RequestState {
    job: Job,
    overwrite: bool,
}

impl From<Request> for RequestState {
    fn from(r: Request) -> Self {
        Self {
            job: r.job,
            overwrite: r.overwrite,
        }
    }
}

#[derive(Serialize)]
struct State {
    input: String,
    output: String,
    overwrite: bool,
    preset: Presets,
    container: Containers,
    format: Format,
    compression_level: Level,
    bitrate: Bitrate,
    copy: Copy,
    sample_rate: Rate,
    channels: ChannelsState,
    adjust: Adjust,
    processing_note: Option<String>,
    notes: Vec<String>,
    problems: Vec<ProblemState>,
    command_line: Option<String>,
    can_queue: bool,
    request: RequestState,
}

fn problem_state(p: &Problem) -> ProblemState {
    let (kind, row) = match p {
        Problem::NoOutput => ("no_output", None),
        Problem::OutputIsInput => ("output_is_input", None),
        Problem::OutputExists => ("output_exists", None),
        Problem::Extension => ("extension", None),
        Problem::Source(_) => ("source", None),
        Problem::Adjust { row, .. } => ("adjust", Some(row_id(*row))),
    };
    ProblemState {
        kind,
        row,
        message: p.message(),
    }
}

fn state(s: &ConvertSheet) -> State {
    let enabled = s.processing_enabled();
    let problems = s.problems(&exists);
    State {
        input: s.draft().job.inputs[0].path.clone(),
        output: s.output().to_owned(),
        overwrite: s.overwrite(),
        preset: Presets {
            value: s.preset().map(|p| p.id),
            options: PRESETS
                .iter()
                .map(|p| Id {
                    id: p.id,
                    label: p.label.to_owned(),
                })
                .collect(),
        },
        container: Containers {
            value: container_id(s.container()),
            label: s.container_label(s.container()),
            options: Container::ALL
                .iter()
                .map(|c| ContainerOption {
                    id: container_id(*c),
                    label: s.container_label(*c),
                    extension: c.extension(),
                })
                .collect(),
        },
        format: Format {
            value: format_id(s.format()),
            label: format_label(s.format()),
            visible: s.format_visible(),
            enabled: s.format_enabled(),
            hint: s.hint(),
            options: s
                .format_options()
                .into_iter()
                .map(|f| Id {
                    id: format_id(f),
                    label: format_label(f),
                })
                .collect(),
        },
        compression_level: Level {
            value: s.level(),
            visible: s.level_visible(),
            enabled: s.level_enabled(),
            options: std::iter::once(None).chain((0..=8).map(Some)).collect(),
        },
        bitrate: Bitrate {
            value: s.bitrate(),
            label: s.bitrate_label(s.bitrate()),
            visible: s.bitrate_visible(),
            enabled: s.bitrate_enabled(),
            options: s
                .bitrate_options()
                .into_iter()
                .map(|b| Labelled {
                    value: b,
                    label: s.bitrate_label(b),
                })
                .collect(),
        },
        copy: Copy {
            value: s.copy(),
            available: s.copy_available(),
        },
        sample_rate: Rate {
            value: s.sample_rate(),
            label: ConvertSheet::rate_label(s.sample_rate()),
            enabled,
            options: s
                .rate_options()
                .into_iter()
                .map(|r| Labelled {
                    value: r,
                    label: ConvertSheet::rate_label(r),
                })
                .collect(),
        },
        channels: ChannelsState {
            value: s.channels().map(ToString::to_string),
            label: ConvertSheet::channel_label(&s.channels().cloned()),
            enabled,
            options: s
                .channel_options()
                .into_iter()
                .map(|c| Labelled {
                    label: ConvertSheet::channel_label(&c),
                    value: c.map(|c| c.to_string()),
                })
                .collect(),
        },
        adjust: Adjust {
            open: s.adjust_open(),
            enabled,
            summary: s.adjust_summary(),
            level_mode: mode_id(s.level_mode()),
            level_value: s.adjust_text(AdjustField::Level).to_owned(),
            trim_start: s.adjust_text(AdjustField::Start).to_owned(),
            trim_end: s.adjust_text(AdjustField::End).to_owned(),
            fade_in: s.adjust_text(AdjustField::FadeIn).to_owned(),
            fade_out: s.adjust_text(AdjustField::FadeOut).to_owned(),
            problems: RowProblems {
                level: s.adjust_problem(AdjustRow::Level),
                trim: s.adjust_problem(AdjustRow::Trim),
                fade: s.adjust_problem(AdjustRow::Fade),
            },
        },
        processing_note: s
            .processing_disabled_reason()
            .or_else(|| s.processing_hint()),
        notes: s.notes(),
        can_queue: problems.is_empty(),
        problems: problems.iter().map(problem_state).collect(),
        // As the desktop: a path that is taken hides nothing of what would run.
        command_line: s.build(&|_| false).ok().and_then(|r| command_line(&r).ok()),
        request: s.draft().into(),
    }
}

fn lock(m: &Mutex<ConvertSheet>) -> MutexGuard<'_, ConvertSheet> {
    // A panic mid-change leaves a sheet whose setters all completed or none ran.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Sheet {
    /// The sheet for the file at `path`; a file the inspector cannot read has none, as on the
    /// desktop, and the refusal is its problem text.
    pub fn open(path: &str) -> Result<Self, Failure> {
        let entry = inspect::entry(path);
        if let InputState::Failed(why) = &entry.state {
            return Err(Failure::refused(why.clone()));
        }
        Ok(Self {
            sheet: Mutex::new(ConvertSheet::for_input(&entry)),
        })
    }

    /// Applies the changes `json` holds, all or none.
    pub fn apply(&self, json: &str) -> Result<(), Failure> {
        let changes = parse_changes(json)?;
        let mut sheet = lock(&self.sheet);
        for c in changes {
            apply(&mut sheet, c);
        }
        Ok(())
    }

    pub fn state_json(&self) -> String {
        serde_json::to_string(&state(&lock(&self.sheet))).unwrap_or_default()
    }

    /// The request the sheet makes, or its problems in its own words.
    pub fn request(&self) -> Result<Request, Failure> {
        lock(&self.sheet).build(&exists).map_err(|problems| {
            let words: Vec<String> = problems.iter().map(Problem::message).collect();
            Failure::refused(words.join(" "))
        })
    }

    /// The draft request, whatever the problems.
    pub fn draft(&self) -> Request {
        lock(&self.sheet).draft()
    }
}
