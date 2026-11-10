//! The convert sheet: what the user has chosen, and the `Request` it makes.
//! Choices map one to one onto CLI arguments (see the parity suite).

use crate::command::Request;
use crate::inputs::InputEntry;
use crossfade_core::SampleFormat;
use crossfade_graph::{AudioSettings, Job, JobInput, JobOutput};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatChoice {
    SameAsSource,
    Format(SampleFormat),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    pub format: FormatChoice,
}

pub const PRESETS: &[Preset] = &[
    Preset {
        id: "same",
        label: "Same as source",
        format: FormatChoice::SameAsSource,
    },
    Preset {
        id: "cd",
        label: "16-bit (CD)",
        format: FormatChoice::Format(SampleFormat::S16),
    },
    Preset {
        id: "studio",
        label: "24-bit",
        format: FormatChoice::Format(SampleFormat::S24),
    },
    Preset {
        id: "float",
        label: "32-bit float",
        format: FormatChoice::Format(SampleFormat::F32),
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    NoOutput,
    OutputIsInput,
    OutputExists,
    NotWav,
}

impl Problem {
    pub fn message(self) -> &'static str {
        match self {
            Self::NoOutput => "Choose where to save the converted file.",
            Self::OutputIsInput => "The output would replace the input file.",
            Self::OutputExists => "That file already exists. Turn on replace to overwrite it.",
            Self::NotWav => "Only .wav output is supported for now.",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConvertSheet {
    input: String,
    output: String,
    // Once the user has typed a path, format changes stop renaming it.
    output_edited: bool,
    format: FormatChoice,
    overwrite: bool,
}

fn default_output(input: &str, format: FormatChoice) -> String {
    let path = Path::new(input);
    let stem = path
        .file_stem()
        .map_or_else(|| "output".to_owned(), |s| s.to_string_lossy().into_owned());
    let tag = match format {
        FormatChoice::SameAsSource => "copy",
        FormatChoice::Format(f) => f.name(),
    };
    path.with_file_name(format!("{stem}-{tag}.wav"))
        .to_string_lossy()
        .into_owned()
}

impl ConvertSheet {
    pub fn for_input(entry: &InputEntry) -> Self {
        let format = FormatChoice::SameAsSource;
        Self {
            output: default_output(&entry.path, format),
            input: entry.path.clone(),
            output_edited: false,
            format,
            overwrite: false,
        }
    }

    pub fn apply_preset(&mut self, id: &str) {
        if let Some(preset) = PRESETS.iter().find(|p| p.id == id) {
            self.set_format(preset.format);
        }
    }

    pub fn set_format(&mut self, format: FormatChoice) {
        self.format = format;
        if !self.output_edited {
            self.output = default_output(&self.input, format);
        }
    }

    pub fn set_output(&mut self, path: String) {
        self.output = path;
        self.output_edited = true;
    }

    pub fn set_overwrite(&mut self, overwrite: bool) {
        self.overwrite = overwrite;
    }

    pub fn preset(&self) -> Option<&'static Preset> {
        PRESETS.iter().find(|p| p.format == self.format)
    }

    pub fn output(&self) -> &str {
        &self.output
    }

    pub fn format(&self) -> FormatChoice {
        self.format
    }

    pub fn overwrite(&self) -> bool {
        self.overwrite
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
        let is_wav = Path::new(&self.output)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("wav"));
        if !is_wav {
            problems.push(Problem::NotWav);
        }
        problems
    }

    pub fn build(&self, exists: &dyn Fn(&str) -> bool) -> Result<Request, Vec<Problem>> {
        let problems = self.problems(exists);
        if !problems.is_empty() {
            return Err(problems);
        }
        Ok(Request {
            job: Job {
                version: Job::VERSION,
                inputs: vec![JobInput {
                    path: self.input.clone(),
                }],
                outputs: vec![JobOutput {
                    path: self.output.clone(),
                    audio: AudioSettings {
                        sample_format: match self.format {
                            FormatChoice::SameAsSource => None,
                            FormatChoice::Format(f) => Some(f),
                        },
                    },
                }],
            },
            overwrite: self.overwrite,
        })
    }
}
