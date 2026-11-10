use super::*;
use crate::settings::fixture::{Demo, error_of, load, position, try_load};

#[test]
fn font_features_read_booleans_and_integers() {
    let settings = load(
        r#"{ "buffer_font_features": { "calt": false, "ss01": true, "cv01": 7 },
             "terminal": {} }"#,
    );
    let features = &settings.buffer_font_features;
    assert_eq!(features.get("calt"), Some(0));
    assert_eq!(features.get("ss01"), Some(1));
    assert_eq!(features.get("cv01"), Some(7));
    assert_eq!(features.get("liga"), None);
    assert_eq!(features.len(), 3);
    assert!(!features.is_empty());
    assert_eq!(
        features.iter().collect::<Vec<_>>(),
        vec![("calt", 0), ("cv01", 7), ("ss01", 1)]
    );
    assert!(!features.ligatures_enabled());
    assert!(FontFeatures::new().ligatures_enabled());

    let liga_off = load(r#"{ "buffer_font_features": { "liga": 0 } }"#);
    assert!(!liga_off.buffer_font_features.ligatures_enabled());
    let calt_on = load(r#"{ "buffer_font_features": { "calt": true } }"#);
    assert!(calt_on.buffer_font_features.ligatures_enabled());
}

#[test]
fn font_features_collect_and_insert() {
    let mut features: FontFeatures = [("calt".to_string(), 0)].into_iter().collect();
    features.insert("ss01", 1);
    assert_eq!(features.get("ss01"), Some(1));
    assert_eq!(features.len(), 2);
}

#[test]
fn font_features_reject_bad_tags_and_values() {
    let err = error_of(r#"{ "buffer_font_features": { "ligatures": true } }"#);
    assert!(err.to_string().contains("four ASCII"), "{err}");
    let err = error_of("{\n \"buffer_font_features\": { \"calt\": \"off\" }\n}");
    assert!(err.to_string().contains("true, false"), "{err}");
    assert_eq!(position(&err).0, 2);
    let err = error_of(r#"{ "buffer_font_features": { "calt": -1 } }"#);
    assert!(err.to_string().contains("non-negative"), "{err}");
}

#[test]
fn line_height_has_three_forms_and_zeds_custom_object() {
    let height = |json: &str| load(json).buffer_line_height;
    assert_eq!(
        height(r#"{ "buffer_line_height": "comfortable" }"#),
        LineHeight::Comfortable
    );
    assert_eq!(
        height(r#"{ "buffer_line_height": "standard" }"#),
        LineHeight::Standard
    );
    assert_eq!(
        height(r#"{ "buffer_line_height": 1.45 }"#),
        LineHeight::Custom(1.45)
    );
    assert_eq!(
        height(r#"{ "buffer_line_height": 2 }"#),
        LineHeight::Custom(2.0)
    );
    assert_eq!(
        height(r#"{ "buffer_line_height": { "custom": 1.5 } }"#),
        LineHeight::Custom(1.5)
    );

    assert_eq!(LineHeight::Comfortable.value(), 1.618);
    assert_eq!(LineHeight::Standard.value(), 1.3);
    assert_eq!(LineHeight::Custom(1.5).value(), 1.5);

    for bad in [
        r#"{ "buffer_line_height": "airy" }"#,
        r#"{ "buffer_line_height": 0 }"#,
        r#"{ "buffer_line_height": -1.2 }"#,
        r#"{ "buffer_line_height": { "custom": 0 } }"#,
        r#"{ "buffer_line_height": { "size": 1.2 } }"#,
        r#"{ "buffer_line_height": {} }"#,
        r#"{ "buffer_line_height": true }"#,
    ] {
        assert!(try_load(bad).is_err(), "{bad} should be rejected");
    }
}

#[test]
fn out_of_range_values_are_errors_with_positions() {
    let err = error_of("{\n  \"buffer_font_weight\": 950\n}");
    assert!(err.to_string().contains("100 to 900"), "{err}");
    assert_eq!(position(&err), (2, 25));

    let err = error_of("{\n  \"buffer_font_size\": 0\n}");
    assert!(err.to_string().contains("positive"), "{err}");
    assert_eq!(position(&err).0, 2);

    let err = error_of(r#"{ "buffer_font_size": "big" }"#);
    assert!(err.to_string().contains("expected a font size"), "{err}");
}

/// A size or height is read as an f64, checked, then stored as an f32, so 1e300 once
/// passed the check and loaded as infinity, and 1e-300 as zero.
#[test]
fn sizes_and_heights_that_do_not_fit_an_f32_are_errors() {
    for bad in [
        r#"{ "buffer_font_size": 1e300 }"#,
        r#"{ "buffer_font_size": 3.5e38 }"#,
        r#"{ "terminal": { "font_size": 1e39 } }"#,
        r#"{ "buffer_font_size": 1e-300 }"#,
        r#"{ "buffer_font_size": 1e-46 }"#,
        r#"{ "buffer_line_height": 1e300 }"#,
        r#"{ "buffer_line_height": { "custom": 1e-300 } }"#,
        r#"{ "terminal": { "line_height": 1e300 } }"#,
    ] {
        let err = try_load(bad).expect_err(bad);
        assert!(err.to_string().contains("out of range"), "{bad}: {err}");
    }
    // The largest and smallest an f32 holds still load, as finite positive numbers.
    let settings = load(r#"{ "buffer_font_size": 3.4e38, "terminal": { "font_size": 2e-45 } }"#);
    assert!(settings.buffer_font_size.is_finite() && settings.buffer_font_size > 0.0);
    let small = settings.terminal.font_size.unwrap();
    assert!(small.is_finite() && small > 0.0);
}

#[test]
fn optional_font_keys_read_null_as_unset() {
    let settings = load(r#"{ "terminal": { "font_size": 12, "font_weight": 700 } }"#);
    assert_eq!(settings.terminal.font_size, Some(12.0));
    assert_eq!(settings.terminal.font_weight, Some(700.0));
    let unset = load(r#"{ "terminal": { "font_size": null, "font_weight": null } }"#);
    assert_eq!(unset.terminal.font_size, None);
    assert_eq!(unset.terminal.font_weight, None);
}

#[test]
fn settings_with_fonts_round_trip_through_json() {
    let demo = load(
        r#"{ "buffer_line_height": { "custom": 1.7 },
             "buffer_font_features": { "ss01": 1, "calt": 0 } }"#,
    );
    let json = serde_json::to_value(&demo).unwrap();
    let back: Demo = serde_json::from_value(json).unwrap();
    assert_eq!(back, demo);
}

#[test]
fn a_font_spec_scales_its_line_height() {
    let spec = FontSpec {
        family: None,
        size: 10.0,
        weight: 400.0,
        features: FontFeatures::new(),
        fallbacks: Vec::new(),
        line_height: 1.5,
    };
    assert_eq!(spec.line_height_px(), 15.0);
}
