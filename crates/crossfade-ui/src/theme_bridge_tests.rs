use appconfig::{Appearance, Theme as ZedTheme, ThemeRegistry};

use crate::prelude::*;
use crate::settings::{BUNDLED_THEMES, THEME_DEFAULTS};
use crate::theme_bridge::{
    SYSTEM_UI_FAMILY, UiFont, ZedColors, apply_kit_theme, contrast_ratio, hex, kit_theme_config,
    to_hsla,
};

fn registry() -> ThemeRegistry {
    ThemeRegistry::with_bundled(BUNDLED_THEMES, THEME_DEFAULTS).expect("bundled themes parse")
}

fn ui_font(size: f32) -> UiFont {
    UiFont { family: None, size }
}

fn install(theme: &ZedTheme, ui: &UiFont, cx: &mut App) {
    let lookup = |key: &str| theme.color(key).map(to_hsla);
    let config = kit_theme_config(
        &ZedColors {
            name: &theme.name,
            dark: theme.is_dark(),
            lookup: &lookup,
        },
        ui,
    );
    apply_kit_theme(config, cx);
}

/// Channels as bytes: a color that went through a hex string differs in the last bit.
fn bytes(color: Hsla) -> [u8; 4] {
    let rgba = color.to_rgb();
    [rgba.r, rgba.g, rgba.b, rgba.a].map(|v| (v * 255.0).round() as u8)
}

#[track_caller]
fn assert_color(actual: Hsla, theme: &ZedTheme, key: &str) {
    let expected = to_hsla(theme.color(key).unwrap_or_else(|| panic!("no {key}")));
    let (a, e) = (bytes(actual), bytes(expected));
    let close = a.iter().zip(e).all(|(a, e)| a.abs_diff(e) <= 1);
    assert!(
        close,
        "{} {key}: got {a:02x?}, expected {e:02x?}",
        theme.name
    );
}

#[test]
fn hex_round_trips_through_gpui_colors() {
    let color = Hsla::from(rgba(0x3b414dff));
    assert_eq!(hex(color).as_ref(), "#3b414dff");
    assert_eq!(hex(Hsla::from(rgba(0x74ade840))).as_ref(), "#74ade840");
}

#[test]
fn contrast_ratio_matches_wcag() {
    let black = Hsla::from(rgba(0x000000ff));
    let white = Hsla::from(rgba(0xffffffff));
    assert!((contrast_ratio(black, white) - 21.0).abs() < 0.01);
    assert!((contrast_ratio(white, white) - 1.0).abs() < 0.001);
}

#[gpui_test]
fn each_bundled_theme_reaches_the_kit_colors(cx: &mut TestAppContext) {
    cx.update(kit_init);
    let registry = registry();
    for name in ["Crossfade Dark", "Crossfade Light"] {
        let theme = registry.get(name).expect("bundled theme");
        cx.update(|cx| {
            install(theme, &ui_font(14.0), cx);
            let kit = cx.theme();
            assert_color(kit.background, theme, "background");
            assert_color(kit.foreground, theme, "text");
            assert_color(kit.border, theme, "border.variant");
            assert_color(kit.muted_foreground, theme, "text.muted");
            assert_color(kit.primary, theme, "text.accent");
            assert_color(kit.accent, theme, "ghost_element.hover");
            assert_color(kit.list_active, theme, "ghost_element.selected");
            assert_color(kit.selection, theme, "players[0].selection");
            assert_color(kit.ring, theme, "border.focused");
            assert_color(kit.danger, theme, "error");
        });
    }
}

#[gpui_test]
fn the_mode_flag_follows_the_theme(cx: &mut TestAppContext) {
    cx.update(kit_init);
    let registry = registry();
    let dark = registry.get("Crossfade Dark").unwrap();
    let light = registry.get("Crossfade Light").unwrap();
    assert_eq!(dark.appearance, Appearance::Dark);
    cx.update(|cx| {
        install(light, &ui_font(14.0), cx);
        assert!(!cx.theme().is_dark());
        install(dark, &ui_font(14.0), cx);
        assert!(cx.theme().is_dark());
        install(light, &ui_font(14.0), cx);
        assert!(!cx.theme().is_dark());
    });
}

#[gpui_test]
fn the_primary_label_is_whichever_reads_better(cx: &mut TestAppContext) {
    cx.update(kit_init);
    let registry = registry();
    for name in ["Crossfade Dark", "Crossfade Light"] {
        let theme = registry.get(name).unwrap();
        cx.update(|cx| {
            install(theme, &ui_font(14.0), cx);
            let kit = cx.theme();
            let background = to_hsla(theme.color("background").unwrap());
            let text = to_hsla(theme.color("text").unwrap());
            let best =
                if contrast_ratio(background, kit.primary) >= contrast_ratio(text, kit.primary) {
                    background
                } else {
                    text
                };
            assert_eq!(bytes(kit.primary_foreground), bytes(best), "{name}");
        });
    }
}

#[gpui_test]
fn the_ui_font_reaches_gpui_kit(cx: &mut TestAppContext) {
    cx.update(kit_init);
    let registry = registry();
    let theme = registry.get("Crossfade Dark").unwrap();
    cx.update(|cx| {
        install(
            theme,
            &UiFont {
                family: Some("Inter".into()),
                size: 18.0,
            },
            cx,
        );
        assert_eq!(cx.theme().font_size, px(18.));
        assert_eq!(cx.theme().font_family.as_ref(), "Inter");

        // Dropping the family from the settings goes back to the default, not to the
        // last one named.
        install(theme, &ui_font(14.0), cx);
        assert_eq!(cx.theme().font_size, px(14.));
        assert_eq!(cx.theme().font_family.as_ref(), SYSTEM_UI_FAMILY);
    });
}

#[gpui_test]
fn a_dialog_backdrop_reads_on_dark_and_light(cx: &mut TestAppContext) {
    cx.update(kit_init);
    let registry = registry();
    cx.update(|cx| {
        install(registry.get("Crossfade Dark").unwrap(), &ui_font(14.0), cx);
        let dark = cx.theme().overlay.a;
        install(registry.get("Crossfade Light").unwrap(), &ui_font(14.0), cx);
        let light = cx.theme().overlay.a;
        assert!(dark > light, "dark {dark} light {light}");
        assert!(light > 0.2, "a light backdrop must still dim: {light}");
    });
}

#[test]
fn every_bundled_theme_defines_the_keys_the_bridge_reads() {
    // The keys `kit_theme_config` looks up, so a theme edit cannot silently drop one.
    const READ: &[&str] = &[
        "background",
        "text",
        "text.muted",
        "surface.background",
        "elevated_surface.background",
        "panel.background",
        "title_bar.background",
        "status_bar.background",
        "tab_bar.background",
        "tab.inactive_background",
        "tab.active_background",
        "element.background",
        "element.hover",
        "element.active",
        "element.selected",
        "ghost_element.hover",
        "ghost_element.selected",
        "border",
        "border.variant",
        "border.focused",
        "border.selected",
        "text.accent",
        "icon.accent",
        "scrollbar.thumb.background",
        "scrollbar.thumb.hover_background",
        "scrollbar.track.background",
        "players[0].selection",
        "players[0].cursor",
        "success",
        "error",
        "warning",
        "info",
        "editor.background",
        "editor.foreground",
    ];
    let registry = registry();
    assert_eq!(registry.len(), 2);
    for theme in registry.themes() {
        for key in READ {
            assert!(theme.color(key).is_some(), "{} lacks {key}", theme.name);
        }
    }
}
