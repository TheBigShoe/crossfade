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

#[test]
fn help_lists_the_subcommands() {
    let out = run(&["--help"]);
    assert_eq!(out.status.code(), Some(0));
    let help = text(&out.stdout);
    assert!(help.contains("probe"), "{help}");
    assert!(help.contains("convert"), "{help}");
}

#[test]
fn convert_to_an_unknown_container_names_the_extension() {
    let dir = std::env::temp_dir().join(format!("crossfade-cli-ext-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_crossfade"))
        .args(["convert", "in.wav", "-o", "out.mp3"])
        .current_dir(&dir)
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let err = text(&out.stderr);
    assert!(err.starts_with("crossfade: "), "{err}");
    assert!(err.contains("mp3"), "{err}");
    for listed in [".wav", ".flac", ".aiff/.aif/.aifc", ".oga/.ogg"] {
        assert!(err.contains(listed), "{listed} missing from {err}");
    }
}

#[test]
fn convert_help_snapshot() {
    let out = run(&["convert", "--help"]);
    assert_eq!(out.status.code(), Some(0));
    insta::assert_snapshot!(text(&out.stdout));
}

#[test]
fn concat_help_snapshot() {
    let out = run(&["concat", "--help"]);
    assert_eq!(out.status.code(), Some(0));
    insta::assert_snapshot!(text(&out.stdout));
}

#[test]
fn convert_help_documents_the_codec_flags() {
    let help = text(&run(&["convert", "--help"]).stdout);
    for want in [
        "--codec <CODEC>",
        "[possible values: copy, pcm, flac]",
        "--compression-level <COMPRESSION_LEVEL>",
        "0 to 8",
    ] {
        assert!(help.contains(want), "{want} missing from {help}");
    }
}

#[test]
fn codec_and_level_values_are_checked_by_the_parser() {
    for (flag, value) in [
        ("--codec", "mp3"),
        ("--codec", "FLAC"),
        ("--compression-level", "9"),
        ("--compression-level", "fast"),
    ] {
        let out = run(&["convert", "in.wav", "-o", "out.flac", flag, value]);
        assert_eq!(out.status.code(), Some(2), "{flag} {value}");
        assert!(out.stdout.is_empty());
        let err = text(&out.stderr);
        assert!(err.contains(flag) && err.contains(value), "{err}");
    }
}

#[test]
fn convert_requires_an_output() {
    let out = run(&["convert", "in.wav"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("--output"));
}
