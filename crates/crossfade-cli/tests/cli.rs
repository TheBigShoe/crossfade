use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_crossfade"))
        .args(args)
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[test]
fn version_prints_name_and_version_only() {
    let out = run(&["--version"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        text(&out.stdout),
        format!("crossfade {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(out.stderr.is_empty());
}

#[test]
fn version_output_shape_snapshot() {
    let out = run(&["--version"]);
    let shown = text(&out.stdout).replace(env!("CARGO_PKG_VERSION"), "[version]");
    insta::assert_snapshot!(shown, @"crossfade [version]\n");
}

#[test]
fn unknown_flag_exits_2_with_usage_on_stderr() {
    let out = run(&["--definitely-not-a-flag"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    let err = text(&out.stderr);
    assert!(err.contains("--definitely-not-a-flag"));
    assert!(err.contains("Usage: crossfade"));
}

// On Windows the file is `crossfade.exe`; usage text must not follow the file name.
#[test]
fn usage_names_the_tool_not_the_file() {
    let renamed =
        std::env::temp_dir().join(format!("crossfade-renamed-{}.exe", std::process::id()));
    std::fs::copy(env!("CARGO_BIN_EXE_crossfade"), &renamed).unwrap();
    let out = Command::new(&renamed).output().unwrap();
    std::fs::remove_file(&renamed).unwrap();
    let err = text(&out.stderr);
    assert!(err.contains("Usage: crossfade"), "{err}");
    assert!(!err.contains(".exe"), "{err}");
}

#[test]
fn no_arguments_prints_help_to_stderr_and_exits_2() {
    let out = run(&[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    let shown = text(&out.stderr).replace(env!("CARGO_PKG_VERSION"), "[version]");
    insta::assert_snapshot!(shown);
}
