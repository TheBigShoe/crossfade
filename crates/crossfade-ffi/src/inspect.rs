//! What a file is: the inspector's sections and the files list's line, or why it cannot be
//! read.

use crossfade_app::FsIo;
use crossfade_app::inputs::{InputEntry, InputState, Inputs, probe_path};
use crossfade_app::inspector;
use crossfade_core::Limits;
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct Row {
    field: &'static str,
    id: String,
    label: String,
    value: String,
    raw: String,
}

#[derive(Serialize)]
pub(crate) struct Section {
    title: String,
    rows: Vec<Row>,
}

#[derive(Serialize)]
pub(crate) struct File {
    path: String,
    name: String,
    readable: bool,
    problem: Option<String>,
    summary: Option<String>,
    sections: Vec<Section>,
}

/// Probes `path` through the real file system into the app's own entry.
pub(crate) fn entry(path: &str) -> InputEntry {
    let state = probe_path(path, &mut FsIo::new(), &Limits::default());
    let mut inputs = Inputs::new();
    inputs.add(path, state);
    inputs.entries()[0].clone()
}

pub(crate) fn describe(entry: &InputEntry) -> File {
    let (readable, problem, summary, sections) = match &entry.state {
        InputState::Probed(doc, _) => (
            true,
            None,
            Some(inspector::summary(doc)),
            inspector::sections(doc)
                .into_iter()
                .map(|s| Section {
                    title: s.title,
                    rows: s
                        .rows
                        .into_iter()
                        .map(|r| Row {
                            field: r.field,
                            id: r.id,
                            label: r.label,
                            value: r.value,
                            raw: r.raw,
                        })
                        .collect(),
                })
                .collect(),
        ),
        InputState::Failed(why) => (false, Some(why.clone()), None, Vec::new()),
    };
    File {
        path: entry.path.clone(),
        name: entry.name.clone(),
        readable,
        problem,
        summary,
        sections,
    }
}

pub(crate) fn json(path: &str) -> String {
    serde_json::to_string(&describe(&entry(path))).unwrap_or_default()
}
