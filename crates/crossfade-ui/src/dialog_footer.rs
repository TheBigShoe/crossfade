//! The footer of the app's dialogs: a Cancel button and a primary one (Save, Convert).
//!
//! gpui-component's plain `Dialog` draws no buttons of its own: `on_ok` and
//! `button_props` only configure what Enter and Escape do, so a visible footer is one the
//! caller builds with `Dialog::footer`. [`DialogButtons`] is that footer. Its buttons do
//! what the keys do: the primary one dispatches [`Confirm`] and Cancel dispatches
//! [`Cancel`] on the dialog they sit in, so the dialog's `on_ok` (which may veto closing)
//! and `on_cancel` handlers run exactly as they do for Enter and Escape.
//!
//! Each button carries a focus node of its own and dispatches from it: a dispatch from the
//! window's focus would miss the dialog whenever something else held focus at the click.

use std::rc::Rc;

use crate::prelude::*;

/// The id of the primary button, for tests that click it.
pub const OK_BUTTON: &str = "dialog-ok";
/// The id of the Cancel button.
pub const CANCEL_BUTTON: &str = "dialog-cancel";

/// Cancel and a primary button, right-aligned, for `Dialog::footer`.
#[derive(IntoElement)]
pub struct DialogButtons {
    ok: SharedString,
    ok_disabled: bool,
    cancel: SharedString,
    danger: bool,
    on_ok: Option<OkClick>,
}

/// What the primary button does in place of dispatching [`Confirm`].
type OkClick = Rc<dyn Fn(&mut Window, &mut App)>;

impl DialogButtons {
    /// A footer whose primary button says `ok` ("Save", "Convert").
    pub fn new(ok: impl Into<SharedString>) -> Self {
        Self {
            ok: ok.into(),
            ok_disabled: false,
            cancel: "Cancel".into(),
            danger: false,
            on_ok: None,
        }
    }

    /// Name the cancel button for what it keeps ("Keep Converting").
    pub fn cancel_label(mut self, label: impl Into<SharedString>) -> Self {
        self.cancel = label.into();
        self
    }

    /// Style the primary button as the destructive choice.
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    /// Run `handler` when the primary button is clicked, leaving Enter, which dispatches
    /// [`Confirm`], to the dialog's own `on_ok`: a destructive dialog can then make Enter
    /// the safe answer.
    pub fn on_ok_click(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_ok = Some(Rc::new(handler));
        self
    }

    /// Grey out the primary button; Enter is the dialog's own and its `on_ok` decides.
    pub fn ok_disabled(mut self, disabled: bool) -> Self {
        self.ok_disabled = disabled;
        self
    }
}

impl RenderOnce for DialogButtons {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let cancel = Anchor::new("dialog-cancel-anchor", window, cx);
        let ok = Anchor::new("dialog-ok-anchor", window, cx);
        let (danger, on_ok) = (self.danger, self.on_ok);
        DialogFooter::new()
            .child(
                Button::new(CANCEL_BUTTON)
                    .label(self.cancel)
                    .child(cancel.element())
                    .on_click(move |_, window, cx| cancel.dispatch(&Cancel, window, cx)),
            )
            .child(
                Button::new(OK_BUTTON)
                    .when_else(danger, |b| b.danger(), |b| b.primary())
                    .disabled(self.ok_disabled)
                    .label(self.ok)
                    .child(ok.element())
                    .on_click(move |_, window, cx| match &on_ok {
                        Some(handler) => handler(window, cx),
                        None => ok.dispatch(&Confirm { secondary: false }, window, cx),
                    }),
            )
    }
}

/// A focus node inside a button, to dispatch an action from the dialog's own dispatch path.
#[derive(Clone)]
struct Anchor(FocusHandle);

impl Anchor {
    /// Created where the button renders, so the keyed state lives in that button's scope.
    fn new(key: &'static str, window: &mut Window, cx: &mut App) -> Self {
        Self(
            window
                .use_keyed_state(key, cx, |_, cx| cx.focus_handle())
                .read(cx)
                .clone(),
        )
    }

    /// A zero-size, out-of-flow node that tracks the handle: never hovered, so it takes no
    /// focus of its own and stays out of the Tab order.
    fn element(&self) -> impl IntoElement {
        div().absolute().size_0().track_focus(&self.0)
    }

    /// Dispatch `action` from the anchor; from the focused element if it was never drawn.
    fn dispatch(&self, action: &dyn Action, window: &mut Window, cx: &mut App) {
        if self.0.is_focused(window) || self.0.contains(&self.0, window) {
            self.0.dispatch_action(action, window, cx);
        } else {
            window.dispatch_action(action.boxed_clone(), cx);
        }
    }
}
