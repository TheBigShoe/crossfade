//! The convert sheet: a dialog over `crossfade_app::sheet::ConvertSheet`. The model holds
//! every choice and decides what is a problem; this view forwards edits to it, draws what
//! it says and keeps the two selects and the path field in step with it.

use crossfade_app::sheet::{Container, ConvertSheet, FormatChoice, PRESETS};
use crossfade_app::{Request, command_line};
use crossfade_core::SampleFormat;

use crate::chrome::{LABEL_SIZE, icon_button};
use crate::dialog_footer::DialogButtons;
use crate::prelude::*;
use crate::workspace::Exists;

/// Every sample-format item there can be, in order; the model says which are offered.
const FORMATS: [(FormatChoice, &str); 7] = [
    (FormatChoice::SameAsSource, "Same as source"),
    (FormatChoice::Format(SampleFormat::U8), "8-bit unsigned"),
    (FormatChoice::Format(SampleFormat::S16), "16-bit integer"),
    (FormatChoice::Format(SampleFormat::S24), "24-bit integer"),
    (FormatChoice::Format(SampleFormat::S32), "32-bit integer"),
    (FormatChoice::Format(SampleFormat::F32), "32-bit float"),
    (FormatChoice::Format(SampleFormat::F64), "64-bit float"),
];

/// The level select's items: the encoder's default, then FLAC's 0 to 8.
const LEVELS: [&str; 10] = [
    "Default",
    "0 \u{b7} Fastest",
    "1",
    "2",
    "3",
    "4",
    "5",
    "6",
    "7",
    "8 \u{b7} Smallest",
];

fn format_label(choice: FormatChoice) -> &'static str {
    FORMATS
        .iter()
        .find(|(format, _)| *format == choice)
        .map_or("", |(_, label)| label)
}

/// The label column's width.
const LABEL_COLUMN: Pixels = px(88.);

/// What the sheet tells whoever opened it.
pub enum ConvertEvent {
    /// The user confirmed a request with no problems.
    Confirmed(Request),
}

type Choice = Entity<SelectState<Vec<String>>>;

fn choice(labels: Vec<String>, window: &mut Window, cx: &mut Context<ConvertDialog>) -> Choice {
    cx.new(|cx| SelectState::new(labels, None, window, cx).searchable(false))
}

/// The dialog's content view.
pub struct ConvertDialog {
    sheet: ConvertSheet,
    title: String,
    exists: Exists,
    output: Entity<InputState>,
    preset: Choice,
    container: Choice,
    format: Choice,
    level: Choice,
    /// What the format select lists now: the engine's options for this source and container.
    format_items: Vec<FormatChoice>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ConvertEvent> for ConvertDialog {}

impl ConvertDialog {
    pub fn new(
        sheet: ConvertSheet,
        file_name: &str,
        exists: Exists,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let output = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Where to save the converted file")
                .default_value(sheet.output().to_owned())
        });
        let preset = choice(
            PRESETS.iter().map(|p| p.label.to_owned()).collect(),
            window,
            cx,
        );
        let container = choice(
            Container::ALL
                .iter()
                .map(|c| c.label().to_owned())
                .collect(),
            window,
            cx,
        );
        let format_items = sheet.format_options();
        let format = choice(
            format_items
                .iter()
                .map(|c| format_label(*c).to_owned())
                .collect(),
            window,
            cx,
        );
        let level = choice(LEVELS.iter().map(|l| (*l).to_owned()).collect(), window, cx);
        let subscriptions = vec![
            // `set_value` stays silent, so a change event is always the user typing.
            cx.subscribe_in(&output, window, |this, input, event, window, cx| {
                if matches!(event, InputEvent::Change) {
                    let text = input.read(cx).value().to_string();
                    this.sheet.set_output(text);
                    // A typed extension can change the container and what it offers.
                    this.sync_choices(window, cx);
                }
            }),
            cx.subscribe_in(
                &preset,
                window,
                |this, _, event: &SelectEvent<Vec<String>>, window, cx| {
                    if let SelectEvent::Confirm(Some(label)) = event
                        && let Some(preset) = PRESETS.iter().find(|p| p.label == label)
                    {
                        this.sheet.apply_preset(preset.id);
                        this.sync(window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &container,
                window,
                |this, _, event: &SelectEvent<Vec<String>>, window, cx| {
                    if let SelectEvent::Confirm(Some(label)) = event
                        && let Some(container) = Container::ALL.iter().find(|c| c.label() == label)
                    {
                        this.sheet.set_container(*container);
                        this.sync(window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &format,
                window,
                |this, _, event: &SelectEvent<Vec<String>>, window, cx| {
                    if let SelectEvent::Confirm(Some(label)) = event
                        && let Some(choice) = this
                            .format_items
                            .iter()
                            .find(|c| format_label(**c) == label)
                    {
                        this.sheet.set_format(*choice);
                        this.sync(window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &level,
                window,
                |this, _, event: &SelectEvent<Vec<String>>, window, cx| {
                    if let SelectEvent::Confirm(Some(label)) = event
                        && let Some(at) = LEVELS.iter().position(|l| l == label)
                    {
                        this.sheet
                            .set_level(at.checked_sub(1).and_then(|l| u8::try_from(l).ok()));
                        this.sync(window, cx);
                    }
                },
            ),
        ];
        let mut dialog = Self {
            sheet,
            title: format!("Convert {file_name}"),
            exists,
            output,
            preset,
            container,
            format,
            level,
            format_items,
            _subscriptions: subscriptions,
        };
        dialog.sync(window, cx);
        dialog
    }

    /// The dialog's title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The output path the sheet holds.
    pub fn output(&self, _: &App) -> String {
        self.sheet.output().to_owned()
    }

    /// Whether the sheet will replace an existing output file.
    pub fn overwrite(&self) -> bool {
        self.sheet.overwrite()
    }

    /// What the sample-format select shows.
    pub fn format_label(&self, _: &App) -> &'static str {
        format_label(self.sheet.format())
    }

    /// What the container select shows.
    pub fn container_label(&self, _: &App) -> &'static str {
        self.sheet.container().label()
    }

    /// What the level select shows.
    pub fn level_label(&self, _: &App) -> &'static str {
        LEVELS[self.sheet.level().map_or(0, |l| usize::from(l) + 1)]
    }

    /// The items of the select called `id`, in order.
    pub fn select_labels(&self, id: &str, _: &App) -> Vec<String> {
        match id {
            "sheet-preset" => PRESETS.iter().map(|p| p.label.to_owned()).collect(),
            "sheet-container" => Container::ALL
                .iter()
                .map(|c| c.label().to_owned())
                .collect(),
            "sheet-format" => self
                .format_items
                .iter()
                .map(|c| format_label(*c).to_owned())
                .collect(),
            "sheet-level" => LEVELS.iter().map(|l| (*l).to_owned()).collect(),
            _ => Vec::new(),
        }
    }

    /// The item of the select called `id` that is showing, if any.
    pub fn shown_label(&self, id: &str, cx: &App) -> Option<String> {
        match id {
            "sheet-preset" => self.preset_label(cx).map(str::to_owned),
            "sheet-container" => Some(self.container_label(cx).to_owned()),
            "sheet-format" => Some(self.format_label(cx).to_owned()),
            "sheet-level" => Some(self.level_label(cx).to_owned()),
            _ => None,
        }
    }

    /// The line saying why the file will not be saved as asked, if it will not.
    pub fn hint(&self) -> Option<String> {
        self.sheet.hint()
    }

    /// Whether the format select can be used (it cannot while copying).
    pub fn format_enabled(&self) -> bool {
        self.sheet.format_enabled()
    }

    /// Whether the level select shows, and can be used.
    pub fn level_enabled(&self) -> bool {
        self.sheet.level_enabled()
    }

    /// Whether "Copy audio unchanged" is on.
    pub fn copy(&self) -> bool {
        self.sheet.copy()
    }

    /// What the preset select shows; `None` when the format matches no preset.
    pub fn preset_label(&self, _: &App) -> Option<&'static str> {
        self.sheet.preset().map(|preset| preset.label)
    }

    /// The warnings under the fields.
    pub fn problem_messages(&self, _: &App) -> Vec<String> {
        self.sheet
            .problems(&*self.exists)
            .into_iter()
            .map(|problem| problem.message())
            .collect()
    }

    /// Put the focus in the path field.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.output.update(cx, |input, cx| input.focus(window, cx));
    }

    /// Emit the request if the sheet has no problems. Returns whether it did.
    pub fn confirm(&mut self, cx: &mut Context<Self>) -> bool {
        match self.sheet.build(&*self.exists) {
            Ok(request) => {
                cx.emit(ConvertEvent::Confirmed(request));
                true
            }
            Err(_) => {
                cx.notify();
                false
            }
        }
    }

    /// Push the model's state into the path field and the selects.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.sheet.output().to_owned();
        self.output.update(cx, |input, cx| {
            if input.value() != path {
                input.set_value(path, window, cx);
            }
            // A path longer than the field shows its end, where the file name is; the
            // input scrolls back to the start on `set_value`.
            input.set_scroll_offset(point(px(-1_000_000.), px(0.)), cx);
        });
        self.sync_choices(window, cx);
    }

    /// Push the model's state into the selects, leaving the path field alone.
    fn sync_choices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let preset = PRESETS
            .iter()
            .position(|p| Some(p.id) == self.sheet.preset().map(|s| s.id));
        self.preset.update(cx, |select, cx| {
            select.set_selected_index(preset.map(IndexPath::new), window, cx);
        });
        let container = Container::ALL
            .iter()
            .position(|c| *c == self.sheet.container());
        self.container.update(cx, |select, cx| {
            select.set_selected_index(container.map(IndexPath::new), window, cx);
        });
        let options = self.sheet.format_options();
        if options != self.format_items {
            self.format_items = options;
            let labels = self
                .format_items
                .iter()
                .map(|c| format_label(*c).to_owned())
                .collect();
            self.format
                .update(cx, |select, cx| select.set_items(labels, window, cx));
        }
        let format = self
            .format_items
            .iter()
            .position(|c| *c == self.sheet.format());
        self.format.update(cx, |select, cx| {
            select.set_selected_index(format.map(IndexPath::new), window, cx);
        });
        let level = usize::from(self.sheet.level().map_or(0, |l| l + 1));
        self.level.update(cx, |select, cx| {
            select.set_selected_index(Some(IndexPath::new(level)), window, cx);
        });
        cx.notify();
    }

    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = std::path::PathBuf::from(self.sheet.output());
        let directory = current
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_default();
        let answer =
            cx.prompt_for_new_path(&directory, current.file_name().and_then(|n| n.to_str()));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = answer.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                this.sheet.set_output(path.to_string_lossy().into_owned());
                this.sync(window, cx);
            })
            .ok();
        })
        .detach();
    }

    fn row(&self, label: &str, content: impl IntoElement, cx: &App) -> Div {
        h_flex()
            .gap_3()
            .items_center()
            .child(
                div()
                    .flex_none()
                    .w(LABEL_COLUMN)
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(label.to_owned())),
            )
            .child(div().flex_1().min_w_0().child(content))
    }
}

impl Render for ConvertDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, warning, mono) = (
            theme.muted_foreground,
            theme.warning,
            theme.mono_font_family.clone(),
        );
        let problems = self.problem_messages(cx);
        // The command stays up while the sheet has a problem: a file that already exists
        // is a reason to disable the button, not to hide what it would run.
        let command = self
            .sheet
            .build(&|_| false)
            .ok()
            .and_then(|request| command_line(&request).ok());
        let overwrite = self.sheet.overwrite();
        let hint = self.sheet.hint();
        let (copy_available, copy) = (self.sheet.copy_available(), self.sheet.copy());
        v_flex()
            .id("convert-sheet")
            .gap_3()
            .pt_2()
            .child(
                self.row(
                    "Preset",
                    div()
                        .id("sheet-preset")
                        .test_support()
                        .child(Select::new(&self.preset).small().placeholder("Custom")),
                    cx,
                ),
            )
            .child(
                self.row(
                    "Container",
                    div()
                        .id("sheet-container")
                        .test_support()
                        .child(Select::new(&self.container).small()),
                    cx,
                ),
            )
            .child(
                self.row(
                    "Format",
                    div().id("sheet-format").test_support().child(
                        Select::new(&self.format)
                            .small()
                            .placeholder("Not available")
                            .disabled(!self.sheet.format_enabled()),
                    ),
                    cx,
                ),
            )
            .when(self.sheet.level_visible(), |sheet| {
                sheet.child(
                    self.row(
                        "Level",
                        div().id("sheet-level").test_support().child(
                            Select::new(&self.level)
                                .small()
                                .disabled(!self.sheet.level_enabled()),
                        ),
                        cx,
                    ),
                )
            })
            .when_some(hint, |sheet, hint| {
                // Under the selects it explains, across the full width so it stays one line.
                sheet.child(
                    div()
                        .id("sheet-hint")
                        .test_support()
                        .text_size(LABEL_SIZE + px(1.))
                        .text_color(muted)
                        .child(SharedString::from(hint)),
                )
            })
            .child(
                self.row(
                    "Output",
                    h_flex()
                        .gap_1()
                        .child(
                            div()
                                .id("sheet-output")
                                .test_support()
                                .flex_1()
                                .min_w_0()
                                .child(Input::new(&self.output).small()),
                        )
                        .child(
                            icon_button(
                                "sheet-browse",
                                IconName::FolderOpen,
                                "Choose a location",
                                cx,
                            )
                            .on_click(cx.listener(|this, _, window, cx| this.browse(window, cx))),
                        ),
                    cx,
                ),
            )
            .child(
                self.row(
                    "",
                    h_flex()
                        .gap_4()
                        .child(
                            div().id("sheet-overwrite").test_support().child(
                                Switch::new("sheet-overwrite-switch")
                                    .label("Replace existing file")
                                    .checked(overwrite)
                                    .on_click({
                                        let this = cx.entity();
                                        move |checked: &bool, _, cx| {
                                            this.update(cx, |dialog, cx| {
                                                dialog.sheet.set_overwrite(*checked);
                                                cx.notify();
                                            });
                                        }
                                    }),
                            ),
                        )
                        .when(copy_available, |row| {
                            row.child(
                                div().id("sheet-copy").test_support().child(
                                    Switch::new("sheet-copy-switch")
                                        .label("Copy audio unchanged")
                                        .checked(copy)
                                        .on_click({
                                            let this = cx.entity();
                                            move |checked: &bool, window, cx| {
                                                this.update(cx, |dialog, cx| {
                                                    dialog.sheet.set_copy(*checked);
                                                    dialog.sync(window, cx);
                                                });
                                            }
                                        }),
                                ),
                            )
                        }),
                    cx,
                ),
            )
            .when(!problems.is_empty(), |sheet| {
                sheet.child(
                    v_flex()
                        .id("sheet-problems")
                        .test_support()
                        .gap_0p5()
                        .px_2()
                        .py_1p5()
                        .rounded_md()
                        .bg(warning.opacity(0.12))
                        .text_color(warning)
                        .text_size(LABEL_SIZE + px(1.))
                        .children(
                            problems
                                .iter()
                                .map(|m| div().child(SharedString::from(m.clone()))),
                        ),
                )
            })
            .when_some(command, |sheet, command| {
                let copied = command.clone();
                sheet.child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(
                            div()
                                .id("sheet-command")
                                .test_support()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_family(mono)
                                .text_size(LABEL_SIZE)
                                .text_color(muted)
                                .child(SharedString::from(command)),
                        )
                        .child(
                            icon_button("sheet-command-copy", IconName::Copy, "Copy command", cx)
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        copied.clone(),
                                    ));
                                }),
                        ),
                )
            })
            .child(
                div()
                    .mt_2()
                    .child(DialogButtons::new("Add to queue").ok_disabled(!problems.is_empty())),
            )
    }
}
