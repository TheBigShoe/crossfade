//! The pieces the window's chrome is built from, so every panel, toolbar and list shares
//! one rhythm: 28 px headers, 24 px inspector rows, a 24 px status bar, 11 px uppercase labels in the
//! theme's muted text, and icon buttons that are ghost until pressed.

use crate::prelude::*;

/// The height of every list row.
pub const ROW_HEIGHT: Pixels = px(28.);
/// The height of an inspector row.
pub const INSPECTOR_ROW: Pixels = px(24.);
/// The height of a two-line row of the files list.
pub const FILE_ROW_HEIGHT: Pixels = px(40.);
/// The first line of a files row: the name, with the remove button at its end.
pub const FILE_NAME_LINE: Pixels = px(24.);
/// The size of the inspector's title; everything else is content size or `LABEL_SIZE`.
pub const TITLE_SIZE: Pixels = px(15.);
/// The height of a panel header and of the toolbar.
pub const HEADER_HEIGHT: Pixels = px(28.);
/// The height of the status bar.
pub const STATUS_HEIGHT: Pixels = px(24.);
/// The size of a header label and of a chip's text.
pub const LABEL_SIZE: Pixels = px(11.);
/// The side of an icon button in a toolbar, a header or a row.
pub const ICON_BUTTON: Pixels = px(24.);

/// An 11 px uppercase label in the muted text color, for a panel's or a group's name.
pub fn section_label(text: &str, cx: &App) -> Div {
    div()
        .flex_none()
        .text_size(LABEL_SIZE)
        .font_semibold()
        .line_height(px(16.))
        .text_color(cx.theme().muted_foreground)
        .child(SharedString::from(text.to_uppercase()))
}

/// A panel's header row: its label on the left; add the actions with `.child`, and
/// push them right with an `ml_auto` on the first.
pub fn panel_header(title: &str, cx: &App) -> Div {
    h_flex()
        .flex_none()
        .w_full()
        .h(HEADER_HEIGHT)
        .pl_3()
        .pr_1()
        .gap_2()
        .items_center()
        .child(section_label(title, cx))
}

/// A ghost icon button in the muted text color. The tooltip is required: an icon alone
/// does not say what it does.
pub fn icon_button(
    id: impl Into<ElementId>,
    icon: IconName,
    tooltip: impl Into<SharedString>,
    cx: &App,
) -> Button {
    Button::new(id)
        .icon(icon)
        .ghost()
        .small()
        .tooltip(tooltip)
        .text_color(cx.theme().muted_foreground)
}

/// An icon button that shows a state: pressed, in the foreground color, while `on`.
pub fn toggle_button(
    id: impl Into<ElementId>,
    icon: IconName,
    tooltip: impl Into<SharedString>,
    on: bool,
    cx: &App,
) -> Button {
    let button = Button::new(id)
        .icon(icon)
        .ghost()
        .small()
        .tooltip(tooltip)
        .selected(on)
        .toggled(on);
    if on {
        button.text_color(cx.theme().foreground)
    } else {
        button.text_color(cx.theme().muted_foreground)
    }
}
