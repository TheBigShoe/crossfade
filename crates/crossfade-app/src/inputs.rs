//! The opened files and which one is selected.

use crate::sheet::plain;
use crossfade_core::{Error, JobIo, Limits, StreamInfo};
use crossfade_graph::{ProbeDocument, probe};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InputId(u64);

#[derive(Debug, Clone, PartialEq)]
pub enum InputState {
    /// The document for display, and the first stream as the engine's `validate` wants it.
    Probed(ProbeDocument, Option<StreamInfo>),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct InputEntry {
    pub id: InputId,
    pub path: String,
    /// The file name, for lists.
    pub name: String,
    pub state: InputState,
}

impl InputEntry {
    pub fn document(&self) -> Option<&ProbeDocument> {
        match &self.state {
            InputState::Probed(doc, _) => Some(doc),
            InputState::Failed(_) => None,
        }
    }

    pub fn stream(&self) -> Option<&StreamInfo> {
        match &self.state {
            InputState::Probed(_, stream) => stream.as_ref(),
            InputState::Failed(_) => None,
        }
    }
}

/// Free of `Inputs` so a UI can run it off the main thread.
pub fn probe_path(path: &str, io: &mut dyn JobIo, limits: &Limits) -> InputState {
    match probe(path, io, limits) {
        Ok(report) => InputState::Probed(report.document(), report.streams.first().cloned()),
        Err(e) => InputState::Failed(refusal(&e)),
    }
}

/// An engine refusal as the inspector shows it.
pub fn refusal(error: &Error) -> String {
    let Error::Unsupported { what } = error else {
        return error.to_string();
    };
    // The cargo feature is no advice for someone using the app.
    if what.starts_with("aac decoding is not part of this build") {
        return "This build of Crossfade does not include AAC decoding.".into();
    }
    let Some(codec) = what
        .strip_prefix("ogg stream holds ")
        .and_then(|rest| rest.split(';').next())
    else {
        return plain(what);
    };
    let holds = match codec {
        "theora" => "Theora video".to_owned(),
        "speex" => "Speex audio".to_owned(),
        "unknown" => "audio Crossfade does not recognise".to_owned(),
        other => format!("{other} data"),
    };
    format!(
        "This Ogg file holds {holds}, which Crossfade cannot open. Inside Ogg it opens FLAC, Vorbis and Opus audio."
    )
}

#[derive(Debug, Default)]
pub struct Inputs {
    entries: Vec<InputEntry>,
    selected: Option<InputId>,
    next_id: u64,
}

impl Inputs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, path: &str, state: InputState) -> InputId {
        let id = if let Some(entry) = self.entries.iter_mut().find(|e| e.path == path) {
            entry.state = state;
            entry.id
        } else {
            let id = InputId(self.next_id);
            self.next_id += 1;
            let name = Path::new(path)
                .file_name()
                .map_or_else(|| path.to_owned(), |n| n.to_string_lossy().into_owned());
            self.entries.push(InputEntry {
                id,
                path: path.to_owned(),
                name,
                state,
            });
            id
        };
        self.selected = Some(id);
        id
    }

    pub fn remove(&mut self, id: InputId) {
        let Some(at) = self.entries.iter().position(|e| e.id == id) else {
            return;
        };
        self.entries.remove(at);
        if self.selected == Some(id) {
            let neighbour = self.entries.get(at).or_else(|| self.entries.last());
            self.selected = neighbour.map(|e| e.id);
        }
    }

    pub fn select(&mut self, id: InputId) {
        if self.entries.iter().any(|e| e.id == id) {
            self.selected = Some(id);
        }
    }

    pub fn selected(&self) -> Option<&InputEntry> {
        let id = self.selected?;
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn entries(&self) -> &[InputEntry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
