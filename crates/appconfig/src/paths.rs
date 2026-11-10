//! Where the configuration lives on disk.

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The names an app picks for its config directory and project folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppIdentity {
    /// Set to a directory to use it as the config directory on any platform, such as
    /// `CROSSFADE_CONFIG_DIR`. It wins on every OS.
    pub env_var: &'static str,
    /// The folder under `$HOME/.config` on macOS and Linux (Zed uses `~/.config/zed`
    /// on macOS too, so a Zed user finds it where they expect).
    pub unix_dir: &'static str,
    /// The folder under `%APPDATA%` on Windows.
    pub windows_dir: &'static str,
    /// The project-local folder, such as `.crossfade`, whose `settings.json` overrides
    /// the user's. The nearest one walking up from the working directory is used.
    pub project_dir: &'static str,
    /// The folder under the temp directory used when no home directory is found.
    pub fallback_dir: &'static str,
}

/// The operating systems the defaults differ on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Platform {
    MacOs,
    Linux,
    Windows,
}

impl Platform {
    /// The platform this binary was built for. Anything that is not macOS or Windows
    /// counts as Linux.
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else if cfg!(target_os = "windows") {
            Platform::Windows
        } else {
            Platform::Linux
        }
    }
}

/// The user's config directory and the files inside it.
///
/// Where the directory is depends on the [`AppIdentity`]: `~/.config/<unix_dir>` on
/// macOS and Linux, `%APPDATA%\<windows_dir>` on Windows, and the identity's
/// environment variable overrides both.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigPaths {
    /// The config directory.
    pub dir: PathBuf,
    /// `settings.json` in `dir`.
    pub settings: PathBuf,
    /// `keymap.json` in `dir`.
    pub keymap: PathBuf,
    /// `themes/` in `dir`: Zed theme family files, one `*.json` each.
    pub themes: PathBuf,
    /// A project-local `<project_dir>/settings.json`, when one was found.
    pub project_settings: Option<PathBuf>,
}

impl ConfigPaths {
    /// Paths inside `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        Self {
            settings: dir.join("settings.json"),
            keymap: dir.join("keymap.json"),
            themes: dir.join("themes"),
            project_settings: None,
            dir,
        }
    }

    /// The directory for this platform, honoring the identity's environment variable.
    pub fn default_for_platform(identity: &AppIdentity) -> Self {
        Self::from_environment(identity, Platform::current(), &|name| {
            std::env::var_os(name)
        })
    }

    /// Like [`default_for_platform`](Self::default_for_platform) with the platform and
    /// environment supplied, so the rules can be tested on any host.
    pub fn from_environment(
        identity: &AppIdentity,
        platform: Platform,
        env: &dyn Fn(&str) -> Option<OsString>,
    ) -> Self {
        let lookup = |name: &str| env(name).filter(|value| !value.is_empty());
        if let Some(dir) = lookup(identity.env_var) {
            return Self::new(dir);
        }
        let dir = match platform {
            Platform::MacOs | Platform::Linux => lookup("HOME")
                .map(|home| PathBuf::from(home).join(".config").join(identity.unix_dir)),
            Platform::Windows => lookup("APPDATA")
                .map(PathBuf::from)
                .or_else(|| {
                    lookup("USERPROFILE")
                        .map(|profile| PathBuf::from(profile).join("AppData").join("Roaming"))
                })
                .map(|base| base.join(identity.windows_dir)),
        };
        Self::new(dir.unwrap_or_else(|| {
            tracing::warn!("no home directory found; keeping the config in the temp directory");
            std::env::temp_dir().join(identity.fallback_dir)
        }))
    }

    /// Adds the project settings file found by searching up from `cwd`.
    pub fn with_project_from(mut self, identity: &AppIdentity, cwd: &Path) -> Self {
        self.project_settings = Self::project_settings_path(identity, cwd);
        self
    }

    /// The nearest `<project_dir>/settings.json` in `cwd` or any of its ancestors.
    pub fn project_settings_path(identity: &AppIdentity, cwd: &Path) -> Option<PathBuf> {
        cwd.ancestors()
            .map(|dir| dir.join(identity.project_dir).join("settings.json"))
            .find(|candidate| candidate.is_file())
    }

    /// Writes `template` as `settings.json` if it does not exist, creating the config
    /// directory as needed. Returns whether it wrote the file. An existing file is
    /// never touched.
    pub fn ensure_settings_file(&self, template: &str) -> io::Result<bool> {
        self.create_new(&self.settings, template)
    }

    /// Writes `template` as `keymap.json` if it does not exist, creating the config
    /// directory as needed. Returns whether it wrote the file. An existing file is
    /// never touched.
    pub fn ensure_keymap_file(&self, template: &str) -> io::Result<bool> {
        self.create_new(&self.keymap, template)
    }

    fn create_new(&self, path: &Path, contents: impl AsRef<[u8]>) -> io::Result<bool> {
        std::fs::create_dir_all(&self.dir)?;
        match OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(mut file) => {
                file.write_all(contents.as_ref())?;
                Ok(true)
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Ok(false),
            Err(err) => Err(err),
        }
    }
}

#[cfg(test)]
mod tests;
