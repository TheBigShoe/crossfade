//! The app's actions. Their key bindings live in the bundled keymap files.

use crate::prelude::*;

actions!(
    crossfade,
    [
        OpenFiles,
        Convert,
        ToggleQueue,
        OpenSettings,
        OpenKeymap,
        Quit,
        About
    ]
);

/// Which action the workspace saw last; stands in for real behaviour until the screens
/// exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppAction {
    OpenFiles,
    Convert,
    ToggleQueue,
    OpenSettings,
    OpenKeymap,
    About,
}

/// The key context of the main window's root view; bindings that only make sense there
/// are scoped to it.
pub const WORKSPACE_CONTEXT: &str = "Workspace";
