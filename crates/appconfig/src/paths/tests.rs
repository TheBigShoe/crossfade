use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;

use super::*;
use crate::test_util::TempDir;

const APP: AppIdentity = AppIdentity {
    env_var: "TESTAPP_CONFIG_DIR",
    unix_dir: "testapp",
    windows_dir: "TestApp",
    project_dir: ".testapp",
    fallback_dir: "testapp-config",
};

fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
    let map: HashMap<String, OsString> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), OsString::from(v)))
        .collect();
    move |name| map.get(name).cloned()
}

#[test]
fn config_dir_follows_the_platform() {
    let unix = env_of(&[("HOME", "/home/pat")]);
    for platform in [Platform::MacOs, Platform::Linux] {
        let paths = ConfigPaths::from_environment(&APP, platform, &unix);
        assert_eq!(paths.dir, Path::new("/home/pat/.config/testapp"));
        assert_eq!(
            paths.settings,
            Path::new("/home/pat/.config/testapp/settings.json")
        );
        assert_eq!(
            paths.keymap,
            Path::new("/home/pat/.config/testapp/keymap.json")
        );
        assert_eq!(paths.themes, Path::new("/home/pat/.config/testapp/themes"));
        assert_eq!(paths.project_settings, None);
    }

    let windows = env_of(&[("APPDATA", "C:\\Users\\pat\\AppData\\Roaming")]);
    let paths = ConfigPaths::from_environment(&APP, Platform::Windows, &windows);
    assert_eq!(
        paths.dir,
        Path::new("C:\\Users\\pat\\AppData\\Roaming").join("TestApp")
    );
    assert!(paths.settings.ends_with("settings.json"));

    let profile_only = env_of(&[("USERPROFILE", "C:\\Users\\pat")]);
    let paths = ConfigPaths::from_environment(&APP, Platform::Windows, &profile_only);
    assert_eq!(
        paths.dir,
        Path::new("C:\\Users\\pat")
            .join("AppData")
            .join("Roaming")
            .join("TestApp")
    );
}

#[test]
fn the_environment_variable_overrides_every_platform() {
    for platform in [Platform::MacOs, Platform::Linux, Platform::Windows] {
        let env = env_of(&[
            ("TESTAPP_CONFIG_DIR", "/tmp/custom"),
            ("HOME", "/home/pat"),
            ("APPDATA", "C:\\Roaming"),
        ]);
        let paths = ConfigPaths::from_environment(&APP, platform, &env);
        assert_eq!(paths.dir, Path::new("/tmp/custom"));
        assert_eq!(paths.settings, Path::new("/tmp/custom/settings.json"));
        assert_eq!(paths.themes, Path::new("/tmp/custom/themes"));
    }
    // An empty override is ignored.
    let env = env_of(&[("TESTAPP_CONFIG_DIR", ""), ("HOME", "/home/pat")]);
    let paths = ConfigPaths::from_environment(&APP, Platform::Linux, &env);
    assert_eq!(paths.dir, Path::new("/home/pat/.config/testapp"));
}

#[test]
fn with_no_home_the_config_falls_back_to_the_temp_dir() {
    let paths = ConfigPaths::from_environment(&APP, Platform::Linux, &env_of(&[]));
    assert_eq!(paths.dir, std::env::temp_dir().join("testapp-config"));
}

#[test]
fn default_for_platform_names_the_expected_files() {
    let paths = ConfigPaths::default_for_platform(&APP);
    assert_eq!(paths.settings.file_name().unwrap(), "settings.json");
    assert_eq!(paths.keymap.file_name().unwrap(), "keymap.json");
    assert_eq!(paths.themes.file_name().unwrap(), "themes");
    assert_eq!(paths.settings.parent(), Some(paths.dir.as_path()));
}

#[test]
fn project_settings_are_found_by_searching_upward() {
    let root = TempDir::new("project");
    let project_file = root.write("repo/.testapp/settings.json", "{}");
    let deep = root.path().join("repo/firmware/build/out");
    std::fs::create_dir_all(&deep).unwrap();

    assert_eq!(
        ConfigPaths::project_settings_path(&APP, &deep),
        Some(project_file.clone())
    );

    // The nearest file wins.
    let nearer = root.write("repo/firmware/.testapp/settings.json", "{}");
    assert_eq!(
        ConfigPaths::project_settings_path(&APP, &deep),
        Some(nearer.clone())
    );

    // A project directory without the file does not count.
    std::fs::create_dir_all(root.path().join("repo/firmware/build/.testapp")).unwrap();
    assert_eq!(
        ConfigPaths::project_settings_path(&APP, &deep),
        Some(nearer)
    );

    let elsewhere = TempDir::new("no-project");
    // Nothing between here and the filesystem root, as long as the temp dir is not
    // itself inside a project.
    if elsewhere
        .path()
        .ancestors()
        .all(|dir| !dir.join(".testapp").join("settings.json").is_file())
    {
        assert_eq!(
            ConfigPaths::project_settings_path(&APP, elsewhere.path()),
            None
        );
    }

    let paths = ConfigPaths::new(root.path().join("config")).with_project_from(&APP, &deep);
    assert!(paths.project_settings.is_some());
}

#[test]
fn a_directory_named_like_the_project_file_is_not_a_file() {
    let root = TempDir::new("project-dir");
    std::fs::create_dir_all(root.path().join(".testapp/settings.json")).unwrap();
    assert_eq!(ConfigPaths::project_settings_path(&APP, root.path()), None);
}

#[test]
fn ensure_settings_file_writes_the_template_once() {
    let root = TempDir::new("ensure");
    let paths = ConfigPaths::new(root.path().join("nested").join("config"));

    assert!(paths.ensure_settings_file("// template\n{}\n").unwrap());
    assert_eq!(
        std::fs::read_to_string(&paths.settings).unwrap(),
        "// template\n{}\n"
    );

    // A second call leaves the user's file alone.
    std::fs::write(&paths.settings, "{ \"a\": 1 }").unwrap();
    assert!(!paths.ensure_settings_file("// template\n{}\n").unwrap());
    assert_eq!(
        std::fs::read_to_string(&paths.settings).unwrap(),
        "{ \"a\": 1 }"
    );
}

#[test]
fn ensure_keymap_file_writes_the_template_once() {
    let root = TempDir::new("ensure-keymap");
    let paths = ConfigPaths::new(root.path().join("config"));
    assert!(paths.ensure_keymap_file("[\n]\n").unwrap());
    assert_eq!(std::fs::read_to_string(&paths.keymap).unwrap(), "[\n]\n");
    assert!(!paths.ensure_keymap_file("[]").unwrap());
    assert_eq!(std::fs::read_to_string(&paths.keymap).unwrap(), "[\n]\n");
}
