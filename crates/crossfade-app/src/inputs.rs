//! The opened files and which one is selected.

use crossfade_core::{JobIo, Limits};
use crossfade_graph::{ProbeDocument, probe};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InputId(u64);

#[derive(Debug, Clone, PartialEq)]
pub enum InputState {
    Probed(ProbeDocument),
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

/// Free of `Inputs` so a UI can run it off the main thread.
pub fn probe_path(path: &str, io: &mut dyn JobIo, limits: &Limits) -> InputState {
    match probe(path, io, limits) {
        Ok(report) => InputState::Probed(report.document()),
        Err(e) => InputState::Failed(e.to_string()),
    }
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
