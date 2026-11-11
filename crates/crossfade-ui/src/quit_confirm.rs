//! The dialog that asks before quitting discards running or waiting conversions. The
//! sentence comes from the queue model each time it draws, so it stays true while the dialog
//! is open.

use crate::dialog_footer::DialogButtons;
use crate::prelude::*;
use crate::workspace::Workspace;

/// The dialog's title.
pub(crate) const TITLE: &str = "Quit Crossfade?";

/// The dialog's content view.
pub(crate) struct QuitConfirm {
    workspace: WeakEntity<Workspace>,
}

impl QuitConfirm {
    pub(crate) fn new(workspace: WeakEntity<Workspace>) -> Self {
        Self { workspace }
    }
}

impl Render for QuitConfirm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = self
            .workspace
            .read_with(cx, |workspace, _| workspace.quit_prompt())
            .ok()
            .flatten()
            .unwrap_or_default();
        let workspace = self.workspace.clone();
        v_flex()
            .id("quit-confirm")
            .test_support()
            .gap_3()
            .pt_2()
            .child(div().child(SharedString::from(body)))
            .child(
                div().mt_2().child(
                    DialogButtons::new("Quit")
                        .cancel_label("Keep Converting")
                        .danger()
                        .on_ok_click(move |window, cx| {
                            workspace
                                .update(cx, |workspace, cx| workspace.confirm_quit(cx))
                                .ok();
                            window.close_dialog(cx);
                        }),
                ),
            )
    }
}
