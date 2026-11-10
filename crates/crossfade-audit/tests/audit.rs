use crossfade_audit::{Metadata, Reason, Violation, audit};

fn fixture(name: &str) -> Metadata {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let json = std::fs::read_to_string(&path).unwrap();
    Metadata::from_json(&json).unwrap()
}

const ENGINE: &[&str] = &["crossfade-core", "crossfade-format"];

#[test]
fn clean_graph_has_no_violations() {
    assert_eq!(audit(&fixture("clean.json"), ENGINE, &[]), vec![]);
}

#[test]
fn links_package_reachable_from_engine_crate_is_flagged() {
    let violations = audit(&fixture("links.json"), ENGINE, &[]);
    assert_eq!(
        violations,
        vec![Violation {
            package: "zlib-native".into(),
            reason: Reason::Links("z".into()),
            path: vec![
                "crossfade-format".into(),
                "inflate".into(),
                "zlib-native".into()
            ],
        }]
    );
}
