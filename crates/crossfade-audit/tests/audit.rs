use crossfade_audit::{Allowed, Metadata, Reason, Violation, audit};

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

#[test]
fn offenders_behind_dev_dependencies_or_non_engine_crates_are_not_flagged() {
    assert_eq!(audit(&fixture("unreachable.json"), &["crossfade-core"], &[]), vec![]);
}

#[test]
fn a_root_missing_from_the_workspace_is_reported() {
    let violations = audit(&fixture("clean.json"), &["crossfade-typo"], &[]);
    assert_eq!(
        violations,
        vec![Violation {
            package: "crossfade-typo".into(),
            reason: Reason::NotInWorkspace,
            path: vec![],
        }]
    );
}

#[test]
fn native_build_helpers_and_sys_crates_are_flagged() {
    let violations = audit(&fixture("helpers.json"), &["crossfade-core"], &[]);
    let found: Vec<(&str, &Reason)> = violations
        .iter()
        .map(|v| (v.package.as_str(), &v.reason))
        .collect();
    assert_eq!(
        found,
        vec![
            ("cc", &Reason::NativeBuildHelper),
            ("nasm-rs", &Reason::NativeBuildHelper),
            ("libfoo-sys", &Reason::SysCrate),
        ]
    );
}

#[test]
fn allowlisted_packages_are_not_flagged() {
    let allow = [Allowed {
        package: "zlib-native",
        reason: "fixture only",
    }];
    assert_eq!(audit(&fixture("links.json"), ENGINE, &allow), vec![]);
}
