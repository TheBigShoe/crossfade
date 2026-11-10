//! The queue dock on the right and the status bar's view of the queue. The state is
//! `crossfade_app::queue::Queue`; this file words it and draws it.

use std::path::Path;

use crossfade_app::queue::{JobId, JobState, QueueItem};

use crate::chrome::{LABEL_SIZE, icon_button, panel_header};
use crate::prelude::*;
use crate::workspace::Workspace;

/// The dock's width.
pub(crate) const DOCK_WIDTH: Pixels = px(340.);

/// `6,000,000`.
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn output_path(item: &QueueItem) -> &str {
    item.request
        .job
        .outputs
        .first()
        .map_or("", |output| output.path.as_str())
}

/// The output's file name, for rows and the status bar.
fn output_name(item: &QueueItem) -> String {
    let path = output_path(item);
    Path::new(path)
        .file_name()
        .map_or_else(|| path.to_owned(), |n| n.to_string_lossy().into_owned())
}

/// The muted line under a row's name.
pub(crate) fn state_line(state: &JobState) -> String {
    match state {
        JobState::Pending => "Waiting".to_owned(),
        JobState::Running {
            cancelling: true, ..
        } => "Cancelling\u{2026}".to_owned(),
        JobState::Running {
            frames_done,
            frames_total: Some(total),
            ..
        } => format!("{}%", percent(*frames_done, *total)),
        JobState::Running { .. } => "Converting\u{2026}".to_owned(),
        JobState::Finished { frames } => format!("Done \u{b7} {} frames", grouped(*frames)),
        JobState::Failed { message } => format!("Failed: {message}"),
        JobState::Cancelled => "Cancelled".to_owned(),
    }
}

fn percent(done: u64, total: u64) -> u64 {
    if total == 0 {
        100
    } else {
        (u128::from(done) * 100 / u128::from(total)).min(100) as u64
    }
}

impl Workspace {
    /// What the row at `n` says about its job.
    pub fn queue_row_state(&self, n: usize) -> String {
        self.queue
            .items()
            .get(n)
            .map(|item| state_line(&item.state))
            .unwrap_or_default()
    }

    /// The labels of the row's overflow menu.
    pub fn queue_menu_labels(&self, n: usize) -> Vec<&'static str> {
        let mut labels = vec!["Copy as command"];
        if self
            .queue
            .items()
            .get(n)
            .is_some_and(|item| matches!(item.state, JobState::Finished { .. }))
        {
            labels.push("Show in folder");
        }
        labels
    }

    /// The right of the status bar: `1 running · 2 waiting · 1 failed`, empty when the
    /// queue is.
    pub fn status_queue(&self) -> String {
        let s = self.queue.summary();
        [
            (s.running, "running"),
            (s.pending, "waiting"),
            (s.failed, "failed"),
            (s.cancelled, "cancelled"),
            (s.finished, "done"),
        ]
        .into_iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, word)| format!("{count} {word}"))
        .collect::<Vec<_>>()
        .join(" \u{b7} ")
    }

    /// The running job's status text, if one runs.
    pub(crate) fn converting_text(&self) -> Option<String> {
        let item = self
            .queue
            .items()
            .iter()
            .find(|item| matches!(item.state, JobState::Running { .. }))?;
        let name = output_name(item);
        Some(match self.queue.fraction(item.id) {
            Some(fraction) => format!(
                "Converting {name} \u{2014} {}%",
                (fraction * 100.0).floor() as u32
            ),
            None => format!("Converting {name}"),
        })
    }

    fn copy_command(&mut self, id: JobId, cx: &mut Context<Self>) {
        if let Some(line) = self.queue.command_line(id) {
            cx.write_to_clipboard(ClipboardItem::new_string(line));
        }
    }

    fn reveal_output(&mut self, id: JobId, cx: &mut Context<Self>) {
        if let Some(item) = self.queue.get(id) {
            cx.reveal_path(Path::new(output_path(item)));
        }
    }

    pub(crate) fn queue_dock(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted) = (theme.border, theme.muted_foreground);
        let count = self.queue.items().len();
        let rows = self
            .queue
            .items()
            .iter()
            .enumerate()
            .map(|(n, item)| self.queue_row(n, item, cx))
            .collect::<Vec<_>>();
        v_flex()
            .id("queue-dock")
            .test_support()
            .flex_none()
            .w(DOCK_WIDTH)
            .h_full()
            .border_l_1()
            .border_color(border)
            .child(
                panel_header("Queue", cx)
                    .child(
                        div()
                            .text_size(LABEL_SIZE)
                            .text_color(muted)
                            .child(count.to_string()),
                    )
                    .child(
                        icon_button("queue-clear", IconName::BrushCleaning, "Clear finished", cx)
                            .ml_auto()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.queue.clear_done();
                                cx.notify();
                            })),
                    ),
            )
            .child(if rows.is_empty() {
                div()
                    .id("queue-empty")
                    .test_support()
                    .px_3()
                    .py_2()
                    .text_color(muted)
                    .child("No conversions yet")
                    .into_any_element()
            } else {
                v_flex()
                    .id("queue-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows)
                    .into_any_element()
            })
    }

    fn queue_row(&self, n: usize, item: &QueueItem, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, danger, primary, border) = (
            theme.muted_foreground,
            theme.danger,
            theme.primary,
            theme.border,
        );
        let id = item.id;
        let this = cx.entity();
        let running = matches!(item.state, JobState::Running { .. });
        let can_cancel = matches!(
            item.state,
            JobState::Pending
                | JobState::Running {
                    cancelling: false,
                    ..
                }
        );
        let can_retry = matches!(item.state, JobState::Failed { .. } | JobState::Cancelled);
        let failed = matches!(item.state, JobState::Failed { .. });
        let finished = matches!(item.state, JobState::Finished { .. });
        let fraction = self.queue.fraction(id).filter(|_| running);
        let menu_this = this.clone();
        v_flex()
            .id(SharedString::from(format!("queue-row-{n}")))
            .test_support()
            .flex_none()
            .w_full()
            .px_3()
            .py_1()
            .gap_0p5()
            .child(
                h_flex()
                    .h(px(24.))
                    .gap_1()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(SharedString::from(output_name(item))),
                    )
                    .when(can_cancel, |row| {
                        row.child(
                            icon_button(
                                SharedString::from(format!("queue-cancel-{n}")),
                                IconName::CircleStop,
                                "Cancel",
                                cx,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.queue.cancel(id);
                                    this.pump();
                                    cx.notify();
                                },
                            )),
                        )
                    })
                    .when(can_retry, |row| {
                        row.child(
                            icon_button(
                                SharedString::from(format!("queue-retry-{n}")),
                                IconName::RotateCcw,
                                "Retry",
                                cx,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.queue.retry(id);
                                    this.pump();
                                    cx.notify();
                                },
                            )),
                        )
                    })
                    .when(!running, |row| {
                        row.child(
                            icon_button(
                                SharedString::from(format!("queue-remove-{n}")),
                                IconName::X,
                                "Remove",
                                cx,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.queue.remove(id);
                                    cx.notify();
                                },
                            )),
                        )
                    })
                    .child(
                        icon_button(
                            SharedString::from(format!("queue-more-{n}")),
                            IconName::Ellipsis,
                            "More",
                            cx,
                        )
                        .dropdown_menu(move |menu, _, _| {
                            let copy = menu_this.clone();
                            let mut menu = menu.item(
                                PopupMenuItem::new("Copy as command").on_click(move |_, _, cx| {
                                    copy.update(cx, |this, cx| this.copy_command(id, cx));
                                }),
                            );
                            if finished {
                                let reveal = menu_this.clone();
                                menu = menu.item(PopupMenuItem::new("Show in folder").on_click(
                                    move |_, _, cx| {
                                        reveal.update(cx, |this, cx| this.reveal_output(id, cx));
                                    },
                                ));
                            }
                            menu
                        }),
                    ),
            )
            .child(
                div()
                    .truncate()
                    .text_size(LABEL_SIZE)
                    .text_color(if failed { danger } else { muted })
                    .child(SharedString::from(state_line(&item.state))),
            )
            .when_some(fraction, |row, fraction| {
                row.child(
                    div()
                        .id(SharedString::from(format!("queue-progress-{n}")))
                        .test_support()
                        .h(px(2.))
                        .w_full()
                        .rounded_full()
                        .bg(border)
                        .child(
                            div()
                                .h_full()
                                .w(relative(fraction))
                                .rounded_full()
                                .bg(primary),
                        ),
                )
            })
            .into_any_element()
    }
}
