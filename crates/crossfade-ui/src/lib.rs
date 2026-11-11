//! The Crossfade desktop app. Nothing in the engine depends on this crate.
#![forbid(unsafe_code)]

pub mod actions;
pub mod chrome;
pub mod config;
pub mod convert_sheet;
pub mod dialog_footer;
pub mod files;
pub mod icons;
pub mod keymap;
pub mod queue_view;
mod quit_confirm;
pub mod scripted;
pub mod settings;
pub mod theme_bridge;
pub mod workspace;

#[cfg(test)]
mod config_test_support;
#[cfg(test)]
mod config_tests;
#[cfg(test)]
mod files_tests;
#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod gate;
#[cfg(test)]
mod keymap_tests;
#[cfg(test)]
mod queue_tests;
#[cfg(test)]
mod quit_tests;
#[cfg(test)]
mod settings_tests;
#[cfg(test)]
mod sheet_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod theme_bridge_tests;
#[cfg(test)]
mod wiring_tests;
#[cfg(test)]
mod workspace_tests;

pub use convert_sheet::{ConvertDialog, ConvertEvent};
pub use workspace::{Workspace, WorkspaceOptions, init, open_main_window};

pub mod prelude {
    //! The only door to GPUI and gpui-kit. Every other file imports
    //! `crate::prelude::*`.

    // gpui-kit re-exports all of GPUI at its root (including `gpui::prelude`), plus
    // `application()` from the platform crate, `init()` and `open_window()`. The last
    // two get unambiguous names because this crate defines its own `init`.
    pub use gpui_kit::prelude::*;
    pub use gpui_kit::*;
    pub use gpui_kit::{init as kit_init, open_window as kit_open_window};
    // The app's icon set, to register with `Application::with_assets`.
    pub use crate::icons::Assets;
    // The Lucide catalog's names. An icon draws only if the registered asset source has
    // its SVG, which is what `icons::Assets` is for.
    pub use gpui_kit::assets::{Assets as KitAssets, IconName, icon_assets};

    // Headless UI-test helpers; they exist only with gpui-kit's `test-support` feature,
    // which the dev-dependency turns on.
    #[cfg(test)]
    pub use gpui_kit::test::{TestAppContextExt, TestWindowExt};

    // GPUI exports its `test` attribute unconditionally, and a glob import outranks the
    // language prelude, so `use crate::prelude::*` would silently turn every `#[test]`
    // into GPUI's. Re-exporting the built-in by name wins over the glob; GPUI's attribute
    // stays reachable as `gpui_test`.
    pub use ::core::prelude::v1::test;
    pub use gpui_kit::test as gpui_test;

    // Observation hook for tests; inert unless `test-support` is on.
    pub use gpui_kit::TestSupportExt;

    pub use gpui_kit::component::button::{Button, ButtonVariants};
    pub use gpui_kit::component::dialog::{Cancel, Confirm, Dialog, DialogFooter};
    // gpui-kit's theme file types, which the theme bridge fills.
    pub use gpui_kit::component::highlighter::HighlightThemeStyle;
    pub use gpui_kit::component::input::{Input, InputEvent, InputState};
    pub use gpui_kit::component::menu::{DropdownMenu, PopupMenu, PopupMenuItem};
    pub use gpui_kit::component::select::{Select, SelectEvent, SelectState};
    pub use gpui_kit::component::switch::Switch;
    pub use gpui_kit::component::tooltip::Tooltip;
    pub use gpui_kit::component::{
        ActiveTheme, Disableable, Icon, Selectable, Sizable, StyledExt, Theme, ThemeMode, h_flex,
        v_flex,
    };
    pub use gpui_kit::component::{IndexPath, WindowExt};
    pub use gpui_kit::component::{ThemeConfig, ThemeConfigColors};
}
