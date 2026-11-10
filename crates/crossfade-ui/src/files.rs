//! The files list on the left and the inspector in the centre, and the drop target they
//! share. Both render the `crossfade-app` models; nothing here decides what to show.

use crossfade_app::inputs::{InputEntry, InputState};
use crossfade_app::inspector::{self, Section};
use crossfade_graph::ProbeDocument;

use crate::chrome::{LABEL_SIZE, ROW_HEIGHT, icon_button, panel_header, section_label};
use crate::prelude::*;
use crate::workspace::Workspace;

/// The files list's width.
pub(crate) const FILES_WIDTH: Pixels = px(260.);
/// The label column of the inspector.
const LABEL_COLUMN: Pixels = px(120.);

/// The shortcut that opens files, as the empty state names it.
const OPEN_SHORTCUT: &str = if cfg!(target_os = "macos") {
    "\u{2318}O"
} else {
    "Ctrl+O"
};

/// `48 kHz`, `44.1 kHz`.
fn kilohertz(rate: u32) -> String {
    if rate % 1000 == 0 {
        format!("{} kHz", rate / 1000)
    } else {
        let tenths = (f64::from(rate) / 100.0).round() / 10.0;
        format!("{tenths} kHz")
    }
}

/// `1:02.250`, or `1:02:03.250` past an hour.
fn clock(seconds: f64) -> String {
    let millis = (seconds * 1000.0).round() as u64;
    let (total, ms) = (millis / 1000, millis % 1000);
    let (hours, minutes, secs) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{secs:02}.{ms:03}")
    } else {
        format!("{minutes}:{secs:02}.{ms:03}")
    }
}

/// The muted line under a file's name: `48 kHz · stereo · s24 · 1:02.250`.
pub(crate) fn summary(entry: &InputEntry) -> String {
    let InputState::Probed(doc) = &entry.state else {
        return "unreadable".to_owned();
    };
    summary_of(doc)
}

fn summary_of(doc: &ProbeDocument) -> String {
    let Some(stream) = doc.streams.first() else {
        return "no audio".to_owned();
    };
    let mut parts = vec![
        kilohertz(stream.sample_rate),
        stream.layout.clone(),
        stream.sample_format.clone(),
    ];
    parts.extend(stream.duration_seconds.map(clock));
    parts.join(" \u{b7} ")
}

impl Workspace {
    /// The summary line of the file at `index`, as its row shows it.
    pub fn file_summary(&self, index: usize) -> String {
        self.inputs
            .entries()
            .get(index)
            .map(summary)
            .unwrap_or_default()
    }

    /// The selected file's inspector sections; empty for a failed or absent file.
    pub fn inspector_sections(&self) -> Vec<Section> {
        match self.inputs.selected().map(|entry| &entry.state) {
            Some(InputState::Probed(doc)) => inspector::sections(doc),
            _ => Vec::new(),
        }
    }

    /// Why the selected file could not be read, if that is so.
    pub fn inspector_error(&self) -> Option<&str> {
        match self.inputs.selected().map(|entry| &entry.state) {
            Some(InputState::Failed(error)) => Some(error),
            _ => None,
        }
    }

    /// The area between the toolbar and the status bar.
    pub(crate) fn body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let primary = cx.theme().primary;
        let highlight = self.drag_over;
        let content = if self.inputs.is_empty() {
            self.empty_state(cx).into_any_element()
        } else {
            h_flex()
                .size_full()
                .child(self.files_panel(cx))
                .child(self.inspector(cx))
                .into_any_element()
        };
        div()
            .id("body")
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(content)
            .on_drag_move::<ExternalPaths>(cx.listener(|this, _, _, cx| {
                if !this.drag_over {
                    this.drag_over = true;
                    cx.notify();
                }
            }))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.drag_over = false;
                this.add_paths(paths.paths().to_vec(), window, cx);
                cx.notify();
            }))
            .when(highlight, |body| {
                body.child(
                    div()
                        .id("drop-highlight")
                        .test_support()
                        .absolute()
                        .inset_2()
                        .rounded_lg()
                        .border_2()
                        .border_color(primary)
                        .bg(primary.opacity(0.08)),
                )
            })
    }

    fn empty_state(&self, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .id("empty-state")
            .test_support()
            .size_full()
            .items_center()
            .justify_center()
            .gap_1()
            .child(
                div()
                    .text_color(theme.foreground)
                    .child("Drop audio files here"),
            )
            .child(
                div()
                    .text_size(LABEL_SIZE + px(1.))
                    .text_color(theme.muted_foreground)
                    .child(format!("or press {OPEN_SHORTCUT} to choose them")),
            )
    }

    fn files_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let selected = self.inputs.selected().map(|entry| entry.id);
        let rows = self
            .inputs
            .entries()
            .iter()
            .enumerate()
            .map(|(n, entry)| {
                let id = entry.id;
                let failed = matches!(entry.state, InputState::Failed(_));
                let is_selected = selected == Some(id);
                let group: SharedString = format!("file-group-{n}").into();
                h_flex()
                    .id(SharedString::from(format!("file-row-{n}")))
                    .test_support()
                    .group(group.clone())
                    .flex_none()
                    .h(ROW_HEIGHT)
                    .px_3()
                    .gap_2()
                    .items_center()
                    .cursor_pointer()
                    .when(is_selected, |row| row.bg(theme.accent))
                    .when(!is_selected, |row| {
                        row.hover(|style| style.bg(theme.accent.opacity(0.5)))
                    })
                    .child(
                        Icon::new(if failed {
                            IconName::TriangleAlert
                        } else {
                            IconName::FileMusic
                        })
                        .size_4()
                        .flex_none()
                        .text_color(if failed {
                            theme.warning
                        } else {
                            theme.muted_foreground
                        }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .max_w(px(110.))
                            .truncate()
                            .child(SharedString::from(entry.name.clone())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(LABEL_SIZE)
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(summary(entry))),
                    )
                    .child(
                        icon_button(
                            SharedString::from(format!("file-remove-{n}")),
                            IconName::X,
                            "Remove",
                            cx,
                        )
                        .invisible()
                        .group_hover(group, |style| style.visible())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.inputs.remove(id);
                            cx.notify();
                        })),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.inputs.select(id);
                        cx.notify();
                    }))
            })
            .collect::<Vec<_>>();
        v_flex()
            .id("files-panel")
            .test_support()
            .flex_none()
            .w(FILES_WIDTH)
            .h_full()
            .border_r_1()
            .border_color(theme.border)
            .child(
                panel_header("Files", cx).child(
                    div()
                        .ml_auto()
                        .pr_2()
                        .text_size(LABEL_SIZE)
                        .text_color(theme.muted_foreground)
                        .child(self.inputs.entries().len().to_string()),
                ),
            )
            .child(
                v_flex()
                    .id("files-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows),
            )
    }

    fn inspector(&self, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        let Some(entry) = self.inputs.selected() else {
            return div().id("inspector").flex_1().into_any_element();
        };
        let (dir, name) = split_path(&entry.path);
        let body =
            match &entry.state {
                InputState::Failed(error) => h_flex()
                    .id("inspector-error")
                    .test_support()
                    .gap_2()
                    .items_start()
                    .text_color(theme.warning)
                    .child(Icon::new(IconName::TriangleAlert).size_4().flex_none())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(SharedString::from(error.clone())),
                    )
                    .into_any_element(),
                InputState::Probed(doc) => v_flex()
                    .gap_4()
                    .children(inspector::sections(doc).into_iter().enumerate().map(
                        |(s, section)| {
                            v_flex()
                                .gap_1()
                                .child(section_label(&section.title, cx))
                                .children(section.rows.into_iter().map(|row| {
                                    h_flex()
                                        .id(SharedString::from(format!(
                                            "inspector-row-{s}-{}",
                                            row.field
                                        )))
                                        .test_support()
                                        .h(ROW_HEIGHT)
                                        .items_center()
                                        .gap_3()
                                        .child(
                                            div()
                                                .flex_none()
                                                .w(LABEL_COLUMN)
                                                .text_color(theme.muted_foreground)
                                                .child(row.label),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .child(SharedString::from(row.value)),
                                        )
                                }))
                        },
                    ))
                    .into_any_element(),
            };
        v_flex()
            .id("inspector")
            .test_support()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .p_4()
            .gap_4()
            .child(
                v_flex()
                    .gap_0p5()
                    .child(
                        div()
                            .font_semibold()
                            .truncate()
                            .child(SharedString::from(name.to_owned())),
                    )
                    // The directory gives way first, so the file name always shows.
                    .child(
                        h_flex()
                            .text_size(LABEL_SIZE)
                            .text_color(theme.muted_foreground)
                            .child(div().min_w_0().truncate().child(SharedString::from(dir)))
                            .child(div().flex_none().child(SharedString::from(name.to_owned()))),
                    ),
            )
            .child(body)
            .into_any_element()
    }
}

/// The directory with its trailing separator, and the file name.
fn split_path(path: &str) -> (String, &str) {
    match path.rfind(['/', '\\']) {
        Some(at) => (path[..=at].to_owned(), &path[at + 1..]),
        None => (String::new(), path),
    }
}
