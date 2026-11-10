//! The app's actions and their default key bindings.

use crate::prelude::*;

actions!(
    crossfade,
    [OpenFiles, Convert, ToggleQueue, OpenSettings, Quit, About]
);

/// Which action the workspace saw last; stands in for real behaviour until the screens
/// exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppAction {
    OpenFiles,
    Convert,
    ToggleQueue,
    OpenSettings,
    About,
}

/// The key context of the main window's root view; bindings that only make sense there
/// are scoped to it.
pub const WORKSPACE_CONTEXT: &str = "Workspace";

/// The built-in bindings. A keymap file will replace this later.
pub fn bind_default_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-o", OpenFiles, None),
        KeyBinding::new("ctrl-o", OpenFiles, None),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("ctrl-q", Quit, None),
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("ctrl-,", OpenSettings, None),
    ]);
}
