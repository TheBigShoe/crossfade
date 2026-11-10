use std::io;

use super::write_atomic;
use crate::test_util::TempDir;

#[test]
fn creates_missing_directories_and_the_file() {
    let dir = TempDir::new("atomic-create");
    let path = dir.path().join("a").join("b").join("settings.json");
    write_atomic(&path, b"{}").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"{}");
}

#[test]
fn replaces_the_contents_and_leaves_no_temp_file() {
    let dir = TempDir::new("atomic-replace");
    let path = dir.write("settings.json", "old contents, longer than the new");
    write_atomic(&path, b"new").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"new");
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["settings.json"]);
}

#[test]
fn a_path_without_a_file_name_is_invalid_input() {
    let err = write_atomic(std::path::Path::new("/"), b"x").unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn a_failed_rename_cleans_up_its_temp_file() {
    let dir = TempDir::new("atomic-fail");
    // A non-empty directory cannot be replaced by a file.
    let target = dir.path().join("target");
    std::fs::create_dir_all(target.join("inner")).unwrap();
    assert!(write_atomic(&target, b"x").is_err());
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty());
}
