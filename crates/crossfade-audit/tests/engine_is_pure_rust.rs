use crossfade_audit::{ALLOWLIST, ENGINE_CRATES, audit, workspace_metadata};

#[test]
fn engine_is_pure_rust() {
    let metadata = workspace_metadata().expect("cargo metadata on the workspace");
    let violations = audit(&metadata, ENGINE_CRATES, ALLOWLIST);
    assert!(violations.is_empty(), "engine is not pure Rust: {violations:#?}");
}
