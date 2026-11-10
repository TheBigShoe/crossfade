//! The convert sheet: a dialog over `crossfade_app::sheet::ConvertSheet`. The model holds
//! every choice and decides what is a problem; this view forwards edits to it, draws what
//! it says and keeps the two selects and the path field in step with it.

use crossfade_app::sheet::{ConvertSheet, FormatChoice, PRESETS};
use crossfade_app::{Request, command_line};
use crossfade_core::SampleFormat;

use crate::chrome::{LABEL_SIZE, icon_button};
use crate::dialog_footer::DialogButtons;
use crate::prelude::*;
use crate::workspace::Exists;

/// The sample-format select's items, in order; the label is what the select shows.
const FORMATS: [(FormatChoice, &str); 7] = [
    (FormatChoice::SameAsSource, "Same as source"),
    (FormatChoice::Format(SampleFormat::U8), "8-bit unsigned"),
    (FormatChoice::Format(SampleFormat::S16), "16-bit integer"),
    (FormatChoice::Format(SampleFormat::S24), "24-bit integer"),
    (FormatChoice::Format(SampleFormat::S32), "32-bit integer"),
    (FormatChoice::Format(SampleFormat::F32), "32-bit float"),
    (FormatChoice::Format(SampleFormat::F64), "64-bit float"),
];

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
    format: Choice,
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
        let format = choice(
            FORMATS
                .iter()
                .map(|(_, label)| (*label).to_owned())
                .collect(),
            window,
            cx,
        );
        let subscriptions = vec![
            // `set_value` stays silent, so a change event is always the user typing.
            cx.subscribe_in(&output, window, |this, input, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    let text = input.read(cx).value().to_string();
                    this.sheet.set_output(text);
                    cx.notify();
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
                &format,
                window,
                |this, _, event: &SelectEvent<Vec<String>>, window, cx| {
                    if let SelectEvent::Confirm(Some(label)) = event
                        && let Some((format, _)) = FORMATS.iter().find(|(_, l)| l == label)
                    {
                        this.sheet.set_format(*format);
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
            format,
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

    /// What the sample-format select shows.
    pub fn format_label(&self, _: &App) -> &'static str {
        FORMATS
            .iter()
            .find(|(format, _)| *format == self.sheet.format())
            .map_or("", |(_, label)| label)
    }

    /// What the preset select shows; `None` when the format matches no preset.
    pub fn preset_label(&self, _: &App) -> Option<&'static str> {
        self.sheet.preset().map(|preset| preset.label)
    }

    /// The warnings under the fields.
    pub fn problem_messages(&self, _: &App) -> Vec<&'static str> {
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

    /// Push the model's state into the path field and both selects.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.sheet.output().to_owned();
        self.output.update(cx, |input, cx| {
            if input.value() != path {
                input.set_value(path, window, cx);
            }
        });
        let preset = PRESETS
            .iter()
            .position(|p| Some(p.id) == self.sheet.preset().map(|s| s.id));
        self.preset.update(cx, |select, cx| {
            select.set_selected_index(preset.map(IndexPath::new), window, cx);
        });
        let format = FORMATS
            .iter()
            .position(|(format, _)| *format == self.sheet.format());
        self.format.update(cx, |select, cx| {
            select.set_selected_index(format.map(IndexPath::new), window, cx);
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
        let command = if problems.is_empty() {
            self.sheet
                .build(&*self.exists)
                .ok()
                .and_then(|request| command_line(&request).ok())
        } else {
            None
        };
        let overwrite = self.sheet.overwrite();
        v_flex()
            .id("convert-sheet")
            .gap_3()
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
                    "Format",
                    div()
                        .id("sheet-format")
                        .test_support()
                        .child(Select::new(&self.format).small()),
                    cx,
                ),
            )
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
                                .font_family(mono.clone())
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
                    h_flex().child(
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
                    ),
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
                        .children(problems.iter().map(|m| div().child(*m))),
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
                            icon_button("sheet-copy", IconName::Copy, "Copy command", cx).on_click(
                                move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        copied.clone(),
                                    ));
                                },
                            ),
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
