//! The main window's root view.

use crate::actions::AppAction;
use crate::prelude::*;

/// The root view: toolbar, empty centre and status bar.
pub struct Workspace {
    last_action: Option<AppAction>,
}

impl Workspace {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self { last_action: None }
    }

    /// The action most recently dispatched to the window, if any.
    pub fn last_action(&self) -> Option<AppAction> {
        self.last_action
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// Global setup: kit components, actions, key bindings. Call once before opening a window.
pub fn init(cx: &mut App) {
    kit_init(cx);
    crate::actions::bind_default_keys(cx);
}
