//! The main window's root view: a toolbar, the files and inspector regions and a status
//! bar. The screens' rendering lives in sibling modules; this file owns the state.

use std::path::PathBuf;
use std::sync::Arc;

use crossfade_app::Request;
use crossfade_app::inputs::{InputState, Inputs, probe_path};
use crossfade_app::queue::{JobId, Queue, Runner, RunnerEvent};
use crossfade_app::sheet::ConvertSheet;
use crossfade_app::{FsIo, runner::ThreadRunner};
use crossfade_core::{JobIo, Limits};

use crate::actions::{
    About, AppAction, Convert, OpenFiles, OpenKeymap, OpenSettings, Quit, ToggleQueue,
    WORKSPACE_CONTEXT,
};
use crate::chrome::{HEADER_HEIGHT, LABEL_SIZE, STATUS_HEIGHT, icon_button, toggle_button};
use crate::config::{self, Config};
use crate::convert_sheet::{ConvertDialog, ConvertEvent};
use crate::keymap::snapshot_kit_bindings;
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

/// How long a workspace notice stays in the status bar.
const NOTICE_TIME: std::time::Duration = std::time::Duration::from_secs(6);

/// A configuration file the app opens in the user's editor.
#[derive(Clone, Copy)]
enum ConfigFile {
    Settings,
    Keymap,
}

/// The root view.
pub struct Workspace {
    pub(crate) focus: FocusHandle,
    last_action: Option<AppAction>,
    pub(crate) inputs: Inputs,
    io: IoFactory,
    pub(crate) queue: Queue,
    runner: Box<dyn Runner>,
    queue_open: bool,
    exists: Exists,
    convert: Option<Entity<ConvertDialog>>,
    _convert_events: Option<Subscription>,
    /// Set while a drag of files is over the window.
    pub(crate) drag_over: bool,
    /// A short-lived message of the workspace's own (a settings file that would not open),
    /// and which one it is, so an older timer never clears a newer message.
    flash: Option<String>,
    flash_serial: u64,
    /// Redraws when the configuration is installed again.
    _config: Subscription,
    /// Applies the runner's reports as they arrive; ends with the view.
    _events: Task<()>,
}

impl Workspace {
    pub fn new(options: WorkspaceOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        // Actions dispatch along the focus path; the root view holds focus so toolbar
        // clicks and shortcuts reach the same handlers.
        window.focus(&focus, cx);
        let WorkspaceOptions {
            runner,
            events,
            io,
            exists,
        } = options;
        let listening = cx.spawn(async move |this, cx| {
            // Reports from a busy worker pile up between wake-ups; apply them together.
            while let Ok(first) = events.recv().await {
                let mut batch = vec![first];
                while let Ok(more) = events.try_recv() {
                    batch.push(more);
                }
                let applied = this.update(cx, |this, cx| {
                    for (id, event) in batch {
                        this.queue.apply(id, event);
                    }
                    this.pump();
                    cx.notify();
                });
                if applied.is_err() {
                    return;
                }
            }
        });
        Self {
            focus,
            last_action: None,
            inputs: Inputs::new(),
            io,
            queue: Queue::new(),
            runner,
            queue_open: false,
            exists,
            convert: None,
            _convert_events: None,
            drag_over: false,
            flash: None,
            flash_serial: 0,
            _config: cx.observe_global::<Config>(|_, cx| cx.notify()),
            _events: listening,
        }
    }

    /// The conversions, waiting, running and done.
    pub fn queue(&self) -> &Queue {
        &self.queue
    }

    /// Whether the queue dock is showing.
    pub fn queue_open(&self) -> bool {
        self.queue_open
    }

    /// The open convert sheet, if there is one.
    pub fn convert_dialog(&self) -> Option<&Entity<ConvertDialog>> {
        self.convert.as_ref()
    }

    /// Open the convert sheet for the selected file. Nothing happens unless the file was
    /// probed.
    pub fn open_convert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.inputs.selected().filter(|_| self.can_convert()) else {
            return;
        };
        let mut sheet = ConvertSheet::for_input(entry);
        if let Some(config) = cx.try_global::<Config>() {
            let defaults = &config.settings().convert;
            // An id the sheet does not know leaves its own default, "same".
            sheet.apply_preset(&defaults.preset);
            sheet.set_overwrite(defaults.overwrite);
        }
        let name = entry.name.clone();
        let exists = Arc::clone(&self.exists);
        let dialog = cx.new(|cx| ConvertDialog::new(sheet, &name, exists, window, cx));
        self._convert_events = Some(cx.subscribe_in(
            &dialog,
            window,
            |this, _, event: &ConvertEvent, window, cx| {
                let ConvertEvent::Confirmed(request) = event;
                this.convert = None;
                window.close_dialog(cx);
                this.enqueue(request.clone(), cx);
            },
        ));
        self.convert = Some(dialog.clone());
        let this = cx.entity().downgrade();
        let title = SharedString::from(dialog.read(cx).title().to_owned());
        window.open_dialog(cx, move |sheet, _, _| {
            let confirm = dialog.clone();
            let closed = this.clone();
            sheet
                .title(title.clone())
                .w(px(520.))
                .child(dialog.clone())
                // The sheet ends in its own footer; confirming emits the request and the
                // subscription closes the dialog, so this handler never closes it itself.
                .on_ok(move |_, _, cx| {
                    confirm.update(cx, |dialog, cx| dialog.confirm(cx));
                    false
                })
                .on_close(move |_, _, cx| {
                    closed
                        .update(cx, |workspace, _| workspace.convert = None)
                        .ok();
                })
        });
        if let Some(dialog) = &self.convert {
            dialog.update(cx, |dialog, cx| dialog.focus(window, cx));
        }
        cx.notify();
    }

    /// Queue `request`, start it if nothing runs, and show the dock.
    pub fn enqueue(&mut self, request: Request, cx: &mut Context<Self>) {
        self.queue.enqueue(request);
        self.pump();
        self.queue_open = true;
        cx.notify();
    }

    /// Start the next waiting job unless one runs.
    pub(crate) fn pump(&mut self) {
        self.queue.pump(&mut *self.runner);
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

    /// Show `text` in the status bar for a few seconds.
    fn flash(&mut self, text: String, cx: &mut Context<Self>) {
        self.flash_serial += 1;
        let serial = self.flash_serial;
        self.flash = Some(text);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(NOTICE_TIME).await;
            this.update(cx, |this, cx| {
                if this.flash_serial == serial {
                    this.flash = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// What the status bar says in place of its normal text: the workspace's own message,
    /// else the configuration's problem, if either exists.
    pub fn status_notice(&self, cx: &App) -> Option<String> {
        self.flash
            .clone()
            .or_else(|| cx.try_global::<Config>().and_then(Config::notice))
    }

    /// Create the file from its template if need be and open it in the user's editor.
    fn open_config_file(&mut self, file: ConfigFile, cx: &mut Context<Self>) {
        let paths = cx.global::<Config>().paths().clone();
        let (name, opened) = match file {
            ConfigFile::Settings => ("settings.json", config::open_settings_file(&paths)),
            ConfigFile::Keymap => ("keymap.json", config::open_keymap_file(&paths)),
        };
        match opened {
            Ok(path) => config::open_path(&path, cx),
            Err(error) => self.flash(format!("Could not open {name}: {error}"), cx),
        }
    }

    /// The window's background, from the theme.
    pub fn surface_color(&self, cx: &App) -> Hsla {
        cx.theme().background
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
                    self.queue_open,
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
                            .menu("Open Keymap", Box::new(OpenKeymap))
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
            .child(match self.status_notice(cx) {
                Some(notice) => div()
                    .id("status-notice")
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(theme.warning)
                    .child(SharedString::from(notice)),
                None => div()
                    .id("status-text")
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(SharedString::from(self.status_text())),
            })
            .child(
                div()
                    .id("status-queue")
                    .test_support()
                    .flex_none()
                    .pl_3()
                    .child(SharedString::from(self.status_queue())),
            )
    }

    /// The left of the status bar: what is selected.
    pub fn status_text(&self) -> String {
        if let Some(text) = self.converting_text() {
            return text;
        }
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
            .bg(self.surface_color(cx))
            .text_color(theme.foreground)
            .on_action(cx.listener(|this, _: &OpenFiles, window, cx| {
                this.record(AppAction::OpenFiles, cx);
                this.prompt_for_files(window, cx);
            }))
            .on_action(cx.listener(|this, _: &Convert, window, cx| {
                this.record(AppAction::Convert, cx);
                this.open_convert(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleQueue, _, cx| {
                this.queue_open = !this.queue_open;
                this.record(AppAction::ToggleQueue, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| {
                this.record(AppAction::OpenSettings, cx);
                this.open_config_file(ConfigFile::Settings, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenKeymap, _, cx| {
                this.record(AppAction::OpenKeymap, cx);
                this.open_config_file(ConfigFile::Keymap, cx);
            }))
            .on_action(cx.listener(|this, _: &About, _, cx| {
                this.record(AppAction::About, cx);
            }))
            .child(self.toolbar(cx))
            .child(self.body(cx))
            .child(self.status_bar(cx))
    }
}

/// Global setup: kit components, actions, menus and the bundled configuration (so a window
/// is themed and bound before `config::start` runs). Call once, before opening a window.
/// Reads no directory: the bundled config's paths are a placeholder in the temp directory
/// that `config::start` or a test replaces.
pub fn init(cx: &mut App) {
    kit_init(cx);
    // Before anything binds keys of its own: keymap applies rebuild from this snapshot.
    snapshot_kit_bindings(cx);
    cx.on_action(|_: &Quit, cx| cx.quit());
    set_menus(cx);
    let unstarted = appconfig::ConfigPaths::new(std::env::temp_dir().join("crossfade-unstarted"));
    config::install(Config::bundled(unstarted), cx);
}

fn set_menus(cx: &mut App) {
    cx.set_menus([
        Menu {
            name: APP_TITLE.into(),
            items: vec![
                MenuItem::action("About Crossfade", About),
                MenuItem::separator(),
                MenuItem::action("Settings\u{2026}", OpenSettings),
                MenuItem::action("Open Keymap", OpenKeymap),
                MenuItem::separator(),
                MenuItem::action("Quit Crossfade", Quit),
            ],
            disabled: false,
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("Add Files\u{2026}", OpenFiles),
                MenuItem::action("Convert\u{2026}", Convert),
                MenuItem::separator(),
                MenuItem::action("Show or Hide Queue", ToggleQueue),
            ],
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
        // `"mode": "system"` themes follow the OS from here on.
        window
            .observe_window_appearance(|window, cx| {
                let dark = matches!(
                    window.appearance(),
                    WindowAppearance::Dark | WindowAppearance::VibrantDark
                );
                config::set_appearance(dark, cx);
            })
            .detach();
        cx.new(|cx| Workspace::new(WorkspaceOptions::real(), window, cx))
    })?;
    cx.activate(true);
    Ok(workspace)
}
