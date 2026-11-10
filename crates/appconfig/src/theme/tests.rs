use crate::paths::ConfigPaths;
use crate::settings::{ThemeMode, ThemeSelection};
use crate::test_util::TempDir;

use super::*;

/// A theme family in the Zed schema: two themes, two players, four syntax captures, every
/// hex form and the two keys that are not colors.
const FIXTURE: &str = r##"{
  "name": "Fixture Colors",
  "author": "appconfig tests",
  "themes": [
    { "name": "Fixture Night", "appearance": "dark", "style": {
        "background.appearance": "opaque",
        "accents": ["#ff0000", "#00ff00"],
        "background": "#101010",
        "panel.background": "#202020",
        "text": "#eeeeee",
        "error": "#ff0000",
        "border.transparent": "#0000",
        "element.hover": "#12345678",
        "terminal.background": "#0a0b0c",
        "players": [
          { "cursor": "#aabbcc", "selection": "#aabbcc40", "background": "#aabbcc" },
          { "cursor": "#ddeeff", "selection": "#ddeeff40", "background": "#ddeeff" }
        ],
        "syntax": {
          "number": { "color": "#f0a574" },
          "string": { "color": "#a3d98f" },
          "keyword": { "color": "#c586c0", "font_weight": 700 },
          "comment": { "color": "#777777", "font_style": "italic" }
        }
    } },
    { "name": "Fixture Day", "appearance": "light", "style": {
        "background": "#f0f0f0",
        "panel.background": "#e0e0e0",
        "text": "#111111",
        "terminal.background": "#fafafa"
    } }
  ]
}"##;

/// A second bundled file, to show the order and that the app's list is what loads.
const EXTRA: &str = r##"{ "name": "Extra", "themes": [
    { "name": "Extra Dusk", "appearance": "dark", "style": { "text": "#cccccc" } } ] }"##;

const BUNDLED: [(&str, &str); 2] = [("fixture.json", FIXTURE), ("extra.json", EXTRA)];
const BUNDLED_NAMES: [&str; 3] = ["Fixture Night", "Fixture Day", "Extra Dusk"];
const DEFAULTS: ThemeDefaults = ThemeDefaults {
    dark: "Fixture Night",
    light: "Fixture Day",
};

fn fixture() -> ThemeFamily {
    let (family, warnings) = ThemeFamily::parse(FIXTURE, "fixture").expect("fixture parses");
    assert!(warnings.is_empty(), "{warnings:?}");
    family
}

fn fixture_theme(name: &str) -> Theme {
    fixture()
        .themes
        .into_iter()
        .find(|theme| theme.name == name)
        .unwrap_or_else(|| panic!("no fixture theme {name}"))
}

fn hex(text: &str) -> Rgba {
    Rgba::parse_hex(text).unwrap_or_else(|err| panic!("{err}"))
}

fn registry() -> ThemeRegistry {
    ThemeRegistry::with_bundled(&BUNDLED, DEFAULTS).expect("the bundled fixtures are valid")
}

// ---- Color parsing ----

#[test]
fn hex_colors_in_all_four_lengths() {
    assert_eq!(hex("#f80").to_u8(), [0xff, 0x88, 0x00, 0xff]);
    assert_eq!(hex("#f808").to_u8(), [0xff, 0x88, 0x00, 0x88]);
    assert_eq!(hex("#ff8800").to_u8(), [0xff, 0x88, 0x00, 0xff]);
    assert_eq!(hex("#ff880080").to_u8(), [0xff, 0x88, 0x00, 0x80]);
    // Either case, and surrounding whitespace.
    assert_eq!(hex("#FF8800AA"), hex("#ff8800aa"));
    assert_eq!(hex("  #abc \n"), hex("#aabbcc"));
    // Channels are 0.0 to 1.0.
    let color = hex("#ff000080");
    assert_eq!(color.r, 1.0);
    assert_eq!(color.g, 0.0);
    assert!((color.a - 128.0 / 255.0).abs() < 1e-6);
    assert_eq!(hex("#0000"), Rgba::TRANSPARENT);
    assert_eq!(hex("#000"), Rgba::BLACK);
    assert_eq!(hex("#fff"), Rgba::WHITE);
}

#[test]
fn bad_colors_are_errors_not_panics() {
    for bad in [
        "",
        "#",
        "#f",
        "#ff",
        "#fffff",
        "#fffffff",
        "#fffffffff",
        "ff8800",
        "#ggg",
        "#12 34",
        "#é12",
        "#ff88００",
        "rgb(1,2,3)",
        "red",
    ] {
        let err = Rgba::parse_hex(bad).expect_err(bad);
        assert_eq!(err.input, bad);
        assert!(err.to_string().contains("not a color"), "{err}");
    }
}

#[test]
fn colors_format_and_convert() {
    assert_eq!(hex("#FF8800").to_hex(), "#ff8800");
    assert_eq!(hex("#ff880080").to_hex(), "#ff880080");
    assert_eq!(hex("#f80").to_string(), "#ff8800");
    assert_eq!("#123456".parse::<Rgba>().unwrap(), hex("#123456"));
    assert_eq!(hex("#102030").with_alpha(0.5).a, 0.5);
    for text in ["#000000", "#ffffff", "#123456", "#12345678", "#00000000"] {
        assert_eq!(hex(text).to_hex(), text);
    }
    let json = serde_json::to_string(&hex("#12345678")).unwrap();
    assert_eq!(json, "\"#12345678\"");
    assert_eq!(
        serde_json::from_str::<Rgba>(&json).unwrap(),
        hex("#12345678")
    );
    assert!(serde_json::from_str::<Rgba>("\"nope\"").is_err());
}

#[test]
fn alpha_blending() {
    let over = hex("#ffffff80").over(Rgba::BLACK);
    assert!((over.r - 128.0 / 255.0).abs() < 1e-6);
    assert_eq!(over.a, 1.0);
    assert_eq!(Rgba::TRANSPARENT.over(hex("#336699")), hex("#336699"));
    assert_eq!(hex("#336699").over(Rgba::WHITE), hex("#336699"));
    assert_eq!(Rgba::TRANSPARENT.over(Rgba::TRANSPARENT), Rgba::TRANSPARENT);
}

// ---- Theme family files ----

#[test]
fn the_fixture_family_has_its_name_author_and_two_themes() {
    let family = fixture();
    assert_eq!(family.name, "Fixture Colors");
    assert_eq!(family.author, "appconfig tests");
    let night = &family.themes[0];
    let day = &family.themes[1];
    assert_eq!(
        (night.name.as_str(), night.appearance),
        ("Fixture Night", Appearance::Dark)
    );
    assert_eq!(
        (day.name.as_str(), day.appearance),
        ("Fixture Day", Appearance::Light)
    );
    assert!(night.is_dark() && !day.is_dark());
    assert_ne!(
        night.color("panel.background"),
        day.color("panel.background")
    );
    assert_eq!(Appearance::from_dark(true), Appearance::Dark);
    assert_eq!(Appearance::Light.to_string(), "light");
}

#[test]
fn the_fixture_reads_every_hex_form_and_the_nested_tables() {
    let theme = fixture_theme("Fixture Night");
    assert_eq!(theme.color("text"), Some(hex("#eeeeee")));
    assert_eq!(theme.color("error"), Some(hex("#ff0000")));
    assert_eq!(theme.color("border.transparent"), Some(Rgba::TRANSPARENT));
    assert_eq!(
        theme.color("element.hover").map(Rgba::to_u8),
        Some([0x12, 0x34, 0x56, 0x78])
    );
    assert_eq!(theme.color("terminal.background"), Some(hex("#0a0b0c")));
    // The keys that are not colors are not in the style map.
    assert_eq!(theme.color("background.appearance"), None);
    assert_eq!(theme.color("accents"), None);

    assert_eq!(theme.players.len(), 2);
    assert_eq!(theme.player(0).unwrap().cursor, Some(hex("#aabbcc")));
    assert_eq!(theme.player(0).unwrap().selection, Some(hex("#aabbcc40")));
    assert_eq!(theme.player(1).unwrap().background, Some(hex("#ddeeff")));
    assert!(theme.player(2).is_none());
    assert_eq!(theme.color("players[0].cursor"), Some(hex("#aabbcc")));
    assert_eq!(theme.color("players[1].selection"), Some(hex("#ddeeff40")));
    assert_eq!(theme.color("players[2].cursor"), None);
    assert_eq!(theme.color("players[0].nonsense"), None);
    assert_eq!(theme.color("players[x].cursor"), None);

    assert_eq!(theme.syntax.len(), 4);
    assert_eq!(theme.syntax_color("string"), Some(hex("#a3d98f")));
    assert_eq!(theme.color("syntax.number"), Some(hex("#f0a574")));
    assert_eq!(theme.color("syntax.missing"), None);
    let comment = theme.syntax_style("comment").unwrap();
    assert_eq!(comment.font_style, Some(FontStyle::Italic));
    assert_eq!(comment.font_weight, None);
    assert_eq!(
        theme.syntax_style("keyword").unwrap().font_weight,
        Some(700.0)
    );
    assert_eq!(theme.syntax_style("number").unwrap().font_style, None);
}

#[test]
fn color_or_falls_back_only_for_missing_keys() {
    let theme = fixture_theme("Fixture Day");
    let fallback = hex("#010203");
    assert_eq!(
        theme.color_or("text", fallback),
        theme.color("text").unwrap()
    );
    assert_eq!(theme.color_or("no.such.key", fallback), fallback);
    assert_eq!(theme.color_or("syntax.missing", fallback), fallback);
    assert_eq!(
        Theme::new("Empty", Appearance::Dark).color_or("text", fallback),
        fallback
    );
}

#[test]
fn bad_style_values_are_skipped_with_a_warning() {
    let text = r##"{
        "name": "Sloppy", "author": "me",
        "themes": [ { "name": "Sloppy Dark", "appearance": "dark", "style": {
            "text": "#ffffff",
            "text.muted": null,
            "text.disabled": "not a color",
            "border": 12,
            "border.variant": ["#fff"],
            "background": "#101010",
            "background.appearance": "blurred",
            "players": [ { "cursor": "#fff", "selection": "bad", "background": null } ],
            "syntax": { "comment": { "color": "#777", "font_style": "wobbly", "font_weight": 50 },
                        "string": "nope" }
        } } ]
    }"##;
    let (family, warnings) = ThemeFamily::parse(text, "sloppy.json").unwrap();
    let theme = &family.themes[0];
    // The good keys survive.
    assert_eq!(theme.color("text"), Some(hex("#ffffff")));
    assert_eq!(theme.color("background"), Some(hex("#101010")));
    for skipped in ["text.muted", "text.disabled", "border", "border.variant"] {
        assert_eq!(theme.color(skipped), None, "{skipped}");
        assert!(
            warnings.iter().any(|w| w.message.starts_with(skipped)),
            "no warning for {skipped}: {warnings:?}"
        );
    }
    // A null is named as such; `background.appearance` is a known non-color key.
    assert!(
        warnings
            .iter()
            .any(|w| w.message.contains("no value (null)"))
    );
    assert!(
        !warnings
            .iter()
            .any(|w| w.message.contains("background.appearance"))
    );
    // Nested tables warn on bad values but keep the rest of the entry.
    assert_eq!(theme.players[0].cursor, Some(hex("#ffffff")));
    assert_eq!(theme.players[0].selection, None);
    assert!(
        warnings
            .iter()
            .any(|w| w.message.contains("players[0].selection"))
    );
    assert_eq!(theme.syntax_color("comment"), Some(hex("#777777")));
    assert!(warnings.iter().any(|w| w.message.contains("font_style")));
    assert!(warnings.iter().any(|w| w.message.contains("font_weight")));
    assert!(warnings.iter().any(|w| w.message.contains("syntax.string")));

    for warning in &warnings {
        assert_eq!(warning.source, "sloppy.json");
        assert_eq!(warning.theme.as_deref(), Some("Sloppy Dark"));
        assert!(
            warning
                .to_string()
                .starts_with("sloppy.json (Sloppy Dark): ")
        );
    }
}

#[test]
fn unusable_theme_entries_are_skipped_but_the_family_loads() {
    let text = r##"{
        "name": "Mixed", "author": "me",
        "themes": [
            { "appearance": "dark", "style": {} },
            { "name": "No appearance", "style": {} },
            { "name": "Bad appearance", "appearance": "sepia", "style": {} },
            { "name": "No style", "appearance": "dark" },
            42,
            { "name": "Good", "appearance": "light", "style": { "text": "#111" } }
        ]
    }"##;
    let (family, warnings) = ThemeFamily::parse(text, "mixed.json").unwrap();
    assert_eq!(family.themes.len(), 1);
    assert_eq!(family.themes[0].name, "Good");
    assert_eq!(warnings.len(), 5, "{warnings:?}");
}

#[test]
fn theme_files_may_have_comments_and_trailing_commas() {
    let text = r##"// my theme
    {
        "name": "Commented", /* inline */
        "themes": [ { "name": "C", "appearance": "dark", "style": { "text": "#fff", }, }, ],
    }"##;
    let (family, warnings) = ThemeFamily::parse(text, "c.json").unwrap();
    assert!(warnings.is_empty());
    assert_eq!(family.author, "");
    assert_eq!(family.themes[0].color("text"), Some(Rgba::WHITE));
}

#[test]
fn broken_theme_files_are_errors_with_positions() {
    let err = ThemeFamily::parse("{\n  \"name\": ,\n}", "broken.json").unwrap_err();
    match err {
        ThemeError::Invalid { file, line, .. } => {
            assert_eq!(file, "broken.json");
            assert_eq!(line, 2);
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
    for (text, needle) in [
        ("[]", "JSON object"),
        (r#"{ "themes": [] }"#, "\"name\""),
        (r#"{ "name": "x" }"#, "\"themes\""),
        (r#"{ "name": "x", "themes": {} }"#, "\"themes\""),
    ] {
        let err = ThemeFamily::parse(text, "f.json").unwrap_err();
        assert!(matches!(err, ThemeError::Schema { .. }), "{text}");
        assert!(err.to_string().contains(needle), "{err}");
    }
}

// ---- Registry ----

#[test]
fn the_bundled_families_load_in_order_with_the_defaults_in_place() {
    let registry = registry();
    assert!(registry.warnings().is_empty(), "{:?}", registry.warnings());
    assert_eq!(registry.names().collect::<Vec<_>>(), BUNDLED_NAMES);
    assert_eq!(registry.len(), 3);
    assert_eq!(registry.themes().count(), 3);
    assert_eq!(registry.default_for(Appearance::Dark).name, "Fixture Night");
    assert_eq!(registry.default_for(Appearance::Light).name, "Fixture Day");
    assert!(format!("{registry:?}").contains("Extra Dusk"));
}

#[test]
fn bundled_text_that_is_not_a_theme_is_an_error_naming_it() {
    let err = ThemeRegistry::with_bundled(&[("broken.json", "{ nope")], DEFAULTS).unwrap_err();
    assert!(
        matches!(&err, ThemeError::Invalid { file, .. } if file == "broken.json"),
        "{err:?}"
    );
}

#[test]
fn a_default_name_that_is_not_bundled_is_an_error() {
    let defaults = ThemeDefaults {
        dark: "Fixture Night",
        light: "Not There",
    };
    let err = ThemeRegistry::with_bundled(&BUNDLED, defaults).unwrap_err();
    assert!(
        matches!(&err, ThemeError::MissingDefault { name } if name == "Not There"),
        "{err:?}"
    );
    assert!(err.to_string().contains("Not There"));
}

fn load_dir(dir: &std::path::Path) -> ThemeRegistry {
    ThemeRegistry::load(&BUNDLED, DEFAULTS, Some(dir)).unwrap()
}

#[test]
fn user_theme_files_load_and_override_by_name() {
    let dir = TempDir::new("themes");
    dir.write(
        "a-extra.json",
        r##"{ "name": "More", "themes": [
        { "name": "More Light", "appearance": "light", "style": { "text": "#202020" } } ] }"##,
    );
    // Sorted by file name, this replaces the bundled dark theme.
    dir.write(
        "b-override.json",
        r##"{ "name": "Mine", "author": "me", "themes": [
            { "name": "Fixture Night", "appearance": "dark",
              "style": { "terminal.background": "#010203" } } ] }"##,
    );
    dir.write("c-broken.json", "{ not json");
    dir.write("notes.txt", "ignored, wrong extension");
    dir.write("nested/deep.json", FIXTURE);

    let registry = load_dir(dir.path());
    let mut names: Vec<&str> = BUNDLED_NAMES.to_vec();
    names.push("More Light");
    assert_eq!(registry.names().collect::<Vec<_>>(), names);
    assert!(registry.get("More Light").is_some());
    assert!(registry.get("Nope").is_none());

    // The override replaced the bundled theme in place, and only its own keys remain.
    let dark = registry.get("Fixture Night").unwrap();
    assert_eq!(dark.color("terminal.background"), Some(hex("#010203")));
    assert_eq!(dark.color("text"), None);
    assert_eq!(registry.default_for(Appearance::Dark).style.len(), 1);
    // The untouched bundled theme is intact.
    assert!(registry.get("Fixture Day").unwrap().style.len() > 2);

    // The broken file is a warning; replacing a bundled theme is not one.
    let warnings = registry.warnings();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].source.ends_with("c-broken.json"),
        "{:?}",
        warnings[0]
    );
}

#[test]
fn a_later_file_with_the_same_name_wins_and_says_so() {
    let dir = TempDir::new("themes-dupe");
    let theme = |color: &str| {
        format!(
            r#"{{ "name": "F", "themes": [ {{ "name": "Same", "appearance": "dark",
                 "style": {{ "text": "{color}" }} }} ] }}"#
        )
    };
    dir.write("1.json", &theme("#111111"));
    dir.write("2.json", &theme("#222222"));
    let registry = load_dir(dir.path());
    assert_eq!(
        registry.get("Same").unwrap().color("text"),
        Some(hex("#222222"))
    );
    let warnings = registry.warnings();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].message.contains("replaces the theme"));
    assert_eq!(warnings[0].theme.as_deref(), Some("Same"));
}

#[test]
fn a_missing_themes_directory_is_fine() {
    let dir = TempDir::new("no-themes");
    let registry = load_dir(&dir.path().join("themes"));
    assert_eq!(registry.len(), 3);
    assert!(registry.warnings().is_empty());
    let none = ThemeRegistry::load(&BUNDLED, DEFAULTS, None).unwrap();
    assert_eq!(none.len(), 3);
    assert!(!none.is_empty());
}

#[test]
fn load_from_uses_the_config_paths_themes_folder() {
    let dir = TempDir::new("paths-themes");
    let paths = ConfigPaths::new(dir.path());
    dir.write(
        "themes/more.json",
        EXTRA.replace("Extra Dusk", "From Folder").as_str(),
    );
    let registry = ThemeRegistry::load_from(&BUNDLED, DEFAULTS, &paths).unwrap();
    assert!(registry.get("From Folder").is_some());
}

#[test]
fn themes_can_be_added_by_text_one_at_a_time() {
    let mut registry = registry();
    registry
        .add_family_text(
            r##"{ "name": "T", "themes": [ { "name": "Added", "appearance": "light",
                  "style": { "text": "#000" } } ] }"##,
            "added.json",
        )
        .unwrap();
    assert!(registry.get("Added").is_some());
    assert!(registry.add_family_text("[]", "bad.json").is_err());
    registry.add_theme(Theme::new("Bare", Appearance::Dark), "code");
    assert_eq!(registry.get("Bare").unwrap().style.len(), 0);
}

fn selection(mode: ThemeMode) -> ThemeSelection {
    ThemeSelection::Dynamic {
        mode,
        light: Some("Fixture Day".into()),
        dark: Some("Extra Dusk".into()),
    }
}

#[test]
fn resolve_follows_the_mode_and_the_system_appearance() {
    let registry = registry();
    let name = |selection: &ThemeSelection, system_dark: bool| {
        registry.resolve(selection, system_dark).name.clone()
    };

    let system = selection(ThemeMode::System);
    assert_eq!(name(&system, true), "Extra Dusk");
    assert_eq!(name(&system, false), "Fixture Day");

    let light = selection(ThemeMode::Light);
    assert_eq!(name(&light, true), "Fixture Day");
    assert_eq!(name(&light, false), "Fixture Day");

    let dark = selection(ThemeMode::Dark);
    assert_eq!(name(&dark, true), "Extra Dusk");
    assert_eq!(name(&dark, false), "Extra Dusk");

    // A plain name pins one theme whatever the system says.
    let pinned = ThemeSelection::Static("Fixture Day".into());
    assert_eq!(name(&pinned, true), "Fixture Day");
    assert_eq!(name(&pinned, false), "Fixture Day");

    assert!(registry.warnings().is_empty());
}

#[test]
fn resolve_falls_back_to_the_default_theme_and_warns() {
    let registry = registry();
    let missing = ThemeSelection::Dynamic {
        mode: ThemeMode::System,
        light: Some("Gone Light".into()),
        dark: Some("Gone Dark".into()),
    };
    assert_eq!(registry.resolve(&missing, true).name, "Fixture Night");
    assert_eq!(registry.resolve(&missing, false).name, "Fixture Day");
    let warnings = registry.warnings();
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(warnings[0].message.contains("Gone Dark"));
    assert!(warnings[0].message.contains("Fixture Night"));
    assert_eq!(warnings[0].theme.as_deref(), Some("Gone Dark"));
    assert!(warnings[1].message.contains("Gone Light"));

    // The same problem is recorded once however often the theme is resolved.
    for _ in 0..5 {
        registry.resolve(&missing, true);
    }
    assert_eq!(registry.warnings().len(), 2);

    // A missing pinned name follows the system appearance for the fallback.
    let pinned = ThemeSelection::Static("Fixture Missing".into());
    assert_eq!(registry.resolve(&pinned, true).name, "Fixture Night");
    assert_eq!(registry.resolve(&pinned, false).name, "Fixture Day");
    assert_eq!(registry.warnings().len(), 4);

    // The mode picks the appearance of the fallback, not the system.
    let forced_light = ThemeSelection::Dynamic {
        mode: ThemeMode::Light,
        light: Some("Gone".into()),
        dark: Some("Gone".into()),
    };
    assert_eq!(registry.resolve(&forced_light, true).name, "Fixture Day");

    let taken = registry.take_warnings();
    assert_eq!(taken.len(), 5);
    assert!(registry.warnings().is_empty());
}

#[test]
fn a_selection_that_names_no_theme_uses_the_default_without_a_warning() {
    let registry = registry();
    let unnamed = ThemeSelection::Dynamic {
        mode: ThemeMode::System,
        light: None,
        dark: Some("Extra Dusk".into()),
    };
    assert_eq!(registry.resolve(&unnamed, true).name, "Extra Dusk");
    assert_eq!(registry.resolve(&unnamed, false).name, "Fixture Day");
    assert!(registry.warnings().is_empty());
}

#[test]
fn the_registry_can_be_shared_between_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ThemeRegistry>();
    assert_send_sync::<Theme>();
}
