//! The main window's root view: a toolbar, the files and inspector regions and a status
//! bar. The screens' rendering lives in sibling modules; this file owns the state.

use std::path::PathBuf;
use std::sync::Arc;

use crossfade_app::inputs::{InputState, Inputs, probe_path};
use crossfade_app::queue::{JobId, Runner, RunnerEvent};
use crossfade_app::{FsIo, runner::ThreadRunner};
use crossfade_core::{JobIo, Limits};

use crate::actions::{
    About, AppAction, Convert, OpenFiles, OpenSettings, Quit, ToggleQueue, WORKSPACE_CONTEXT,
    bind_default_keys,
};
use crate::chrome::{HEADER_HEIGHT, LABEL_SIZE, STATUS_HEIGHT, icon_button, toggle_button};
use crate::prelude::*;

/// What the window is called.
pub const APP_TITLE: &str = "Crossfade";
/// The platform application id.
pub const APP_ID: &str = "dev.crossfade.Crossfade";

/// Makes the `JobIo` a probe reads through; called on the UI thread, used on a worker.
pub type IoFactory = Arc<dyn Fn() -> Box<dyn JobIo + Send> + Send + Sync>;
/// Whether a path already names a file; the convert sheet asks it to flag overwrites.
pub type Exists = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// What the workspace works through. Tests and screenshots script the runner and its
/// events; [`WorkspaceOptions::real`] is what the app runs on.
pub struct WorkspaceOptions {
    pub runner: Box<dyn Runner>,
    /// What the runner's worker threads report; the workspace applies each to the queue.
    pub events: async_channel::Receiver<(JobId, RunnerEvent)>,
    pub io: IoFactory,
    pub exists: Exists,
}

impl WorkspaceOptions {
    /// Real worker threads over the real file system.
    pub fn real() -> Self {
        let (sender, events) = async_channel::unbounded();
        let runner = ThreadRunner::new(Limits::default(), move |id, event| {
            // The receiver only goes away with the window.
            let _ = sender.try_send((id, event));
        });
        Self {
            runner: Box::new(runner),
            events,
            io: Arc::new(|| Box::new(FsIo::new())),
            exists: Arc::new(|path| std::path::Path::new(path).exists()),
        }
    }
}

/// The root view.
pub struct Workspace {
    pub(crate) focus: FocusHandle,
    last_action: Option<AppAction>,
    pub(crate) inputs: Inputs,
    io: IoFactory,
    /// Set while a drag of files is over the window.
    pub(crate) drag_over: bool,
}

impl Workspace {
    pub fn new(options: WorkspaceOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        // Actions dispatch along the focus path; the root view holds focus so toolbar
        // clicks and shortcuts reach the same handlers.
        window.focus(&focus, cx);
        let WorkspaceOptions { io, .. } = options;
        Self {
            focus,
            last_action: None,
            inputs: Inputs::new(),
            io,
            drag_over: false,
        }
    }

    /// The opened files and which one is selected.
    pub fn inputs(&self) -> &Inputs {
        &self.inputs
    }

    /// Probe `paths` off the main thread and list them in the order given; the last one
    /// becomes the selection.
    pub fn add_paths(&mut self, paths: Vec<PathBuf>, _: &mut Window, cx: &mut Context<Self>) {
        let probes: Vec<_> = paths
            .into_iter()
            .map(|path| {
                let path = path.to_string_lossy().into_owned();
                let mut io = (self.io)();
                let key = path.clone();
                let task =
                    cx.background_spawn(
                        async move { probe_path(&key, &mut *io, &Limits::default()) },
                    );
                (path, task)
            })
            .collect();
        cx.spawn(async move |this, cx| {
            for (path, task) in probes {
                let state = task.await;
                if this
                    .update(cx, |this, cx| {
                        this.inputs.add(&path, state);
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();
    }

    /// Ask the platform for audio files, then [`Self::add_paths`].
    fn prompt_for_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let answer = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Add".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            // Cancelled, or the dialog went away.
            let Ok(Ok(Some(paths))) = answer.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| this.add_paths(paths, window, cx))
                .ok();
        })
        .detach();
    }

    /// The action most recently dispatched to the window, if any.
    pub fn last_action(&self) -> Option<AppAction> {
        self.last_action
    }

    fn record(&mut self, action: AppAction, cx: &mut Context<Self>) {
        self.last_action = Some(action);
        cx.notify();
    }

    fn toolbar(&self, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        let dispatch = |action: Box<dyn Action>, focus: &FocusHandle| {
            let focus = focus.clone();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                focus.dispatch_action(&*action, window, cx);
            }
        };
        let menu_focus = self.focus.clone();
        h_flex()
            .id("toolbar")
            .flex_none()
            .w_full()
            .h(HEADER_HEIGHT + px(8.))
            .px_2()
            .gap_1()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .child(
                icon_button("toolbar-add", IconName::Plus, "Add files", cx)
                    .on_click(dispatch(Box::new(OpenFiles), &self.focus)),
            )
            .child(
                icon_button("toolbar-convert", IconName::Play, "Convert", cx)
                    .disabled(!self.can_convert())
                    .on_click(dispatch(Box::new(Convert), &self.focus)),
            )
            .child(div().flex_1())
            .child(
                toggle_button(
                    "toolbar-queue",
                    IconName::PanelRight,
                    "Toggle queue",
                    false,
                    cx,
                )
                .on_click(dispatch(Box::new(ToggleQueue), &self.focus)),
            )
            .child(
                icon_button("toolbar-settings", IconName::Settings, "Settings", cx)
                    .on_click(dispatch(Box::new(OpenSettings), &self.focus)),
            )
            .child(
                icon_button("toolbar-overflow", IconName::Ellipsis, "More", cx).dropdown_menu(
                    move |menu, _, _| {
                        menu.action_context(menu_focus.clone())
                            .menu("About Crossfade", Box::new(About))
                    },
                ),
            )
    }

    fn status_bar(&self, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .id("status-bar")
            .flex_none()
            .w_full()
            .h(STATUS_HEIGHT)
            .px_3()
            .items_center()
            .border_t_1()
            .border_color(theme.border)
            .text_size(LABEL_SIZE)
            .text_color(theme.muted_foreground)
            .child(
                div()
                    .id("status-text")
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(SharedString::from(self.status_text())),
            )
    }

    /// The left of the status bar: what is selected.
    pub fn status_text(&self) -> String {
        self.inputs
            .selected()
            .map_or_else(|| "Ready".to_owned(), |entry| entry.name.clone())
    }

    /// Convert needs a selected file the probe could read.
    pub(crate) fn can_convert(&self) -> bool {
        self.inputs
            .selected()
            .is_some_and(|entry| matches!(entry.state, InputState::Probed(_)))
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .id("workspace")
            .test_support()
            .key_context(WORKSPACE_CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .on_action(cx.listener(|this, _: &OpenFiles, window, cx| {
                this.record(AppAction::OpenFiles, cx);
                this.prompt_for_files(window, cx);
            }))
            .on_action(cx.listener(|this, _: &Convert, _, cx| {
                this.record(AppAction::Convert, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleQueue, _, cx| {
                this.record(AppAction::ToggleQueue, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| {
                this.record(AppAction::OpenSettings, cx);
            }))
            .on_action(cx.listener(|this, _: &About, _, cx| {
                this.record(AppAction::About, cx);
            }))
            .child(self.toolbar(cx))
            .child(self.body(cx))
            .child(self.status_bar(cx))
    }
}

/// Global setup: kit components, actions, key bindings and menus. Call once, before
/// opening a window.
pub fn init(cx: &mut App) {
    kit_init(cx);
    cx.on_action(|_: &Quit, cx| cx.quit());
    bind_default_keys(cx);
    set_menus(cx);
}

fn set_menus(cx: &mut App) {
    cx.set_menus([
        Menu {
            name: APP_TITLE.into(),
            items: vec![
                MenuItem::action("About Crossfade", About),
                MenuItem::separator(),
                MenuItem::action("Settings\u{2026}", OpenSettings),
                MenuItem::separator(),
                MenuItem::action("Quit Crossfade", Quit),
            ],
            disabled: false,
        },
        Menu {
            name: "File".into(),
            items: vec![MenuItem::action("Add Files\u{2026}", OpenFiles)],
            disabled: false,
        },
    ]);
}

/// Open the main window, centred at 1180x760, with a [`Workspace`] as its root view.
pub fn open_main_window(cx: &mut App) -> Result<Entity<Workspace>> {
    let bounds = Bounds::centered(None, size(px(1180.), px(760.)), cx);
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some(APP_TITLE.into()),
            ..Default::default()
        }),
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(640.), px(400.))),
        app_id: Some(APP_ID.into()),
        ..Default::default()
    };
    let (_, workspace) = kit_open_window(options, cx, |window, cx| {
        cx.new(|cx| Workspace::new(WorkspaceOptions::real(), window, cx))
    })?;
    cx.activate(true);
    Ok(workspace)
}
