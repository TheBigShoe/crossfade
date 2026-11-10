//! The bridge from a Zed theme to gpui-kit's theme, so every kit widget follows the
//! same theme file the app's own views read.
//!
//! gpui-kit keeps its colours in a data-driven [`ThemeConfig`] (keys such as
//! `background`, `primary.background`, `sidebar.background`) and resolves each missing
//! key from related ones. The bridge fills a config from the Zed keys that mean the same
//! thing, leaves the rest to gpui-kit's fallbacks, and installs it with
//! [`Theme::apply_config`] inside [`Theme::update`], which rebuilds the derived styles and
//! repaints every window. The config becomes gpui-kit's light or dark theme, so its own
//! mode changes reload it rather than a bundled one.
//!
//! The UI font family and size ride along as gpui-kit's `font.family` and `font.size`,
//! which the root view turns into the window's rem size.

use std::rc::Rc;

use appconfig::Rgba as ZedRgba;

use crate::prelude::*;

/// gpui's name for the platform's UI face, which is also gpui-kit's default family.
pub const SYSTEM_UI_FAMILY: &str = ".SystemUIFont";

/// The dimming behind a dialog: black at 60% on a dark theme, 35% on a light one. gpui-kit's
/// own default barely shows on a window that is dark to begin with.
const DARK_OVERLAY: &str = "#00000099";
const LIGHT_OVERLAY: &str = "#00000059";

/// A theme colour as a GPUI one.
pub fn to_hsla(color: ZedRgba) -> Hsla {
    Hsla::from(Rgba {
        r: color.r,
        g: color.g,
        b: color.b,
        a: color.a,
    })
}

/// `#rrggbbaa`, the form gpui-kit's theme files use.
pub fn hex(color: Hsla) -> SharedString {
    let rgba = color.to_rgb();
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    SharedString::from(format!(
        "#{:02x}{:02x}{:02x}{:02x}",
        byte(rgba.r),
        byte(rgba.g),
        byte(rgba.b),
        byte(rgba.a)
    ))
}

/// WCAG contrast ratio between two colours, alpha ignored.
pub fn contrast_ratio(a: Hsla, b: Hsla) -> f32 {
    fn luminance(color: Hsla) -> f32 {
        let rgba = color.to_rgb();
        let linear = |c: f32| {
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(rgba.r) + 0.7152 * linear(rgba.g) + 0.0722 * linear(rgba.b)
    }
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// What the interface is drawn in.
#[derive(Clone, Debug, PartialEq)]
pub struct UiFont {
    /// `None` is gpui-kit's default family.
    pub family: Option<String>,
    pub size: f32,
}

/// A Zed theme as the bridge needs it: a name, an appearance and a key lookup.
pub struct ZedColors<'a> {
    pub name: &'a str,
    pub dark: bool,
    pub lookup: &'a dyn Fn(&str) -> Option<Hsla>,
}

impl ZedColors<'_> {
    fn first(&self, keys: &[&str]) -> Option<Hsla> {
        keys.iter().find_map(|key| (self.lookup)(key))
    }

    fn hex(&self, keys: &[&str]) -> Option<SharedString> {
        self.first(keys).map(hex)
    }
}

/// gpui-kit's theme for `zed`, with the UI font.
///
/// | gpui-kit | Zed keys, first set wins |
/// | --- | --- |
/// | `background` | `background` |
/// | `foreground`, `*.foreground` of panels, popovers, buttons | `text` |
/// | `muted.foreground`, `tab.foreground` | `text.muted` |
/// | `muted.background` | `surface.background` |
/// | `popover.background` | `elevated_surface.background` |
/// | `sidebar.background`, `list.background` | `panel.background` |
/// | `title_bar.background` | `title_bar.background` |
/// | `status_bar.background` | `status_bar.background` |
/// | `tab_bar.background`, `tab.background`, `tab.active.background` | `tab_bar.background`, `tab.inactive_background`, `tab.active_background` |
/// | `button.*`, `secondary.*` backgrounds | `element.background`, `element.hover`, `element.active` |
/// | `accent.background`, `list.hover.background` | `ghost_element.hover` |
/// | `list.active.background` | `ghost_element.selected`, `element.selected` |
/// | `border` and the panel borders | `border.variant`, `border` |
/// | `input.border`, `window_border` | `border` |
/// | `ring`, `drag.border` | `border.focused` |
/// | `list.active.border` | `border.selected`, `border.focused` |
/// | `primary.background`, `link` | `text.accent`, `icon.accent` |
/// | `scrollbar.*` | `scrollbar.track.background`, `scrollbar.thumb.background`, `scrollbar.thumb.hover_background` |
/// | `selection.background`, `caret` | `players[0].selection`, `players[0].cursor` |
/// | `success`, `danger`, `warning`, `info` backgrounds | `success`, `error`, `warning`, `info` |
/// | `overlay` (a dialog's backdrop) | none: black at 60% (dark) or 35% (light) |
/// | highlight `editor.background`, `editor.foreground` | `editor.background`, `editor.foreground` |
///
/// The primary foreground has no Zed key: it is whichever of `background` and `text`
/// reads better on the primary colour.
pub fn kit_theme_config(zed: &ZedColors<'_>, ui: &UiFont) -> ThemeConfig {
    let border = zed.hex(&["border.variant", "border"]);
    let text = zed.hex(&["text"]);
    let primary = zed.first(&["text.accent", "icon.accent"]);
    let primary_foreground = primary.and_then(|primary| {
        let candidates = [zed.first(&["background"]), zed.first(&["text"])];
        candidates
            .into_iter()
            .flatten()
            .max_by(|a, b| contrast_ratio(*a, primary).total_cmp(&contrast_ratio(*b, primary)))
            .map(hex)
    });

    // gpui-kit's config types are non-exhaustive, which rules out struct-update syntax.
    let mut c = ThemeConfigColors::default();
    c.background = zed.hex(&["background"]);
    c.foreground = text.clone();
    c.muted = zed.hex(&["surface.background"]);
    c.muted_foreground = zed.hex(&["text.muted"]);
    c.border = border.clone();
    c.input = zed.hex(&["border"]);
    c.ring = zed.hex(&["border.focused"]);
    c.drag_border = zed.hex(&["border.focused"]);
    c.window_border = zed.hex(&["border"]);

    c.accent = zed.hex(&["ghost_element.hover", "element.hover"]);
    c.accent_foreground = text.clone();
    c.button = zed.hex(&["element.background"]);
    c.button_hover = zed.hex(&["element.hover"]);
    c.button_active = zed.hex(&["element.active"]);
    c.button_foreground = text.clone();
    c.secondary = zed.hex(&["element.background"]);
    c.secondary_hover = zed.hex(&["element.hover"]);
    c.secondary_active = zed.hex(&["element.active"]);
    c.secondary_foreground = text.clone();
    c.primary = primary.map(hex);
    c.primary_foreground = primary_foreground;
    c.link = primary.map(hex);

    c.popover = zed.hex(&["elevated_surface.background"]);
    c.popover_foreground = text.clone();
    c.sidebar = zed.hex(&["panel.background"]);
    c.sidebar_foreground = text.clone();
    c.sidebar_border = border.clone();
    c.sidebar_accent = zed.hex(&["ghost_element.hover", "element.hover"]);
    c.sidebar_accent_foreground = text.clone();
    c.list = zed.hex(&["panel.background"]);
    c.list_hover = zed.hex(&["ghost_element.hover", "element.hover"]);
    c.list_active = zed.hex(&["ghost_element.selected", "element.selected"]);
    c.list_active_border = zed.hex(&["border.selected", "border.focused"]);

    c.title_bar = zed.hex(&["title_bar.background"]);
    c.title_bar_border = border.clone();
    c.status_bar = zed.hex(&["status_bar.background"]);
    c.status_bar_border = border;
    c.tab_bar = zed.hex(&["tab_bar.background"]);
    c.tab = zed.hex(&["tab.inactive_background"]);
    c.tab_active = zed.hex(&["tab.active_background"]);
    c.tab_foreground = zed.hex(&["text.muted"]);
    c.tab_active_foreground = text;

    c.scrollbar = zed.hex(&["scrollbar.track.background"]);
    c.scrollbar_thumb = zed.hex(&["scrollbar.thumb.background"]);
    c.scrollbar_thumb_hover = zed.hex(&["scrollbar.thumb.hover_background"]);
    c.selection = zed.hex(&["players[0].selection"]);
    c.caret = zed.hex(&["players[0].cursor"]);

    c.overlay = Some(SharedString::from(if zed.dark {
        DARK_OVERLAY
    } else {
        LIGHT_OVERLAY
    }));

    c.success = zed.hex(&["success"]);
    c.danger = zed.hex(&["error"]);
    c.warning = zed.hex(&["warning"]);
    c.info = zed.hex(&["info"]);

    let mut highlight = HighlightThemeStyle::default();
    highlight.editor_background = zed.first(&["editor.background"]);
    highlight.editor_foreground = zed.first(&["editor.foreground"]);

    let mut config = ThemeConfig::default();
    config.is_default = false;
    config.name = SharedString::from(zed.name.to_owned());
    config.mode = if zed.dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    config.font_size = Some(ui.size);
    // Always set: `apply_config` only overwrites a family that is `Some`, so leaving it
    // out would keep the last family named after the setting is removed.
    config.font_family = Some(ui.family.as_deref().map_or_else(
        || SharedString::new_static(SYSTEM_UI_FAMILY),
        |f| f.to_owned().into(),
    ));
    config.colors = c;
    config.highlight = Some(highlight);
    config
}

/// Install `config` as gpui-kit's theme, switching to its mode, and repaint every
/// window.
pub fn apply_kit_theme(config: ThemeConfig, cx: &mut App) {
    let config = Rc::new(config);
    Theme::update(cx, |theme| theme.apply_config(&config));
}
