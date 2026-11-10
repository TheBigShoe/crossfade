//! The main window's root view: a toolbar, an empty centre and a status bar. Real
//! behaviour comes with the screens; for now actions are only recorded.

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

/// The shortcut that opens files, as the empty state names it.
const OPEN_SHORTCUT: &str = if cfg!(target_os = "macos") {
    "\u{2318}O"
} else {
    "Ctrl+O"
};

/// The root view.
pub struct Workspace {
    focus: FocusHandle,
    last_action: Option<AppAction>,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        // Actions dispatch along the focus path; the root view holds focus so toolbar
        // clicks and shortcuts reach the same handlers.
        window.focus(&focus, cx);
        Self {
            focus,
            last_action: None,
        }
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

    fn empty_state(&self, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .id("empty-state")
            .test_support()
            .flex_1()
            .w_full()
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
            .child(div().id("status-text").test_support().child("Ready"))
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
            .on_action(cx.listener(|this, _: &OpenFiles, _, cx| {
                this.record(AppAction::OpenFiles, cx);
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
            .child(self.empty_state(cx))
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
        cx.new(|cx| Workspace::new(window, cx))
    })?;
    cx.activate(true);
    Ok(workspace)
}
