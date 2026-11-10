//! Settings, theme, fonts and keymap as one GPUI global, loaded from the config directory
//! and applied to the running app.
//!
//! # Flow
//!
//! [`start`] loads everything from a [`ConfigPaths`] (see [`paths_for`]) and installs it
//! with [`install`]: gpui-kit's theme and UI font are replaced through the theme bridge,
//! the key bindings are rebuilt, and the [`Config`] global is set, which notifies every
//! view that called `cx.observe_global::<Config>`. gpui-kit must be initialised first
//! (`Theme` is its global).
//!
//! A [`ConfigWatcher`] reports saves; a foreground task drains its channel and reloads the
//! piece that changed.
//!
//! # Failures
//!
//! Nothing here is fatal. A settings file that does not parse keeps the last good
//! settings (the bundled defaults at startup), a bad keymap keeps the last good bindings,
//! a bad theme file is skipped. Each problem is logged and kept as a [`ConfigProblem`];
//! [`Config::notice`] is the one line for a status bar, and it clears on the next load that
//! succeeds.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use appconfig::{
    ConfigEvent, ConfigPaths, ConfigWatcher, Keymap, Platform, SettingsError, Theme as ZedTheme,
    ThemeRegistry, load_files, load_keymap,
};
use crossbeam_channel::RecvTimeoutError;

use crate::keymap::{self, bundled_keymap_text};
use crate::prelude::*;
use crate::settings::{
    BUNDLED_THEMES, DEFAULT_SETTINGS, IDENTITY, KEYMAP_TEMPLATE, SETTINGS_TEMPLATE, Settings,
    THEME_DEFAULTS, clamp_ui_font_size,
};
use crate::theme_bridge::{UiFont, ZedColors, apply_kit_theme, kit_theme_config, to_hsla};

/// Which part of the configuration a load or a problem is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConfigPiece {
    /// `settings.json` and the project settings.
    Settings,
    /// `keymap.json`.
    Keymap,
    /// The theme files in `themes/`.
    Themes,
    /// The theme the settings select, which may name one no file provides.
    Theme,
    /// Keymap entries GPUI could not bind: an unknown action, a bad keystroke.
    Bindings,
    /// A font family the settings name that is not installed.
    Fonts,
}

impl fmt::Display for ConfigPiece {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ConfigPiece::Settings => "settings",
            ConfigPiece::Keymap => "keymap",
            ConfigPiece::Themes => "themes",
            ConfigPiece::Theme => "theme",
            ConfigPiece::Bindings => "bindings",
            ConfigPiece::Fonts => "fonts",
        })
    }
}

/// A problem from the last load of one piece.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigProblem {
    pub piece: ConfigPiece,
    /// The file the problem is about (the folder, for themes).
    pub file: PathBuf,
    /// One line, naming the file the way a person would.
    pub message: String,
    /// The load failed and something older (or the default) is in use.
    pub is_error: bool,
}

/// The loaded configuration and everything derived from it.
#[derive(Clone)]
pub struct Config {
    paths: ConfigPaths,
    settings: Arc<Settings>,
    /// The bundled bindings followed by the user's.
    keymap: Arc<Keymap>,
    themes: Arc<ThemeRegistry>,
    theme: Arc<ZedTheme>,
    /// The window appearance a `"mode": "system"` selection follows.
    system_dark: bool,
    ui_font: UiFont,
    problems: Vec<ConfigProblem>,
    /// Bumped by every install, so a view can tell a reload from a repeat.
    generation: u64,
}

impl Global for Config {}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("dir", &self.paths.dir)
            .field("theme", &self.theme.name)
            .field("problems", &self.problems)
            .finish_non_exhaustive()
    }
}

impl Config {
    /// The bundled defaults, with nothing read from disk. `paths` is where files would be
    /// read from and where problems say they are.
    pub fn bundled(paths: ConfigPaths) -> Self {
        let themes = Arc::new(bundled_themes());
        let keymap = load_keymap(bundled_keymap_text(Platform::current()), None)
            .expect("the bundled keymap parses");
        let mut config = Self {
            paths,
            settings: Arc::new(Settings::default()),
            keymap: Arc::new(keymap),
            theme: Arc::new(themes.default_for(appconfig::Appearance::Dark).clone()),
            themes,
            system_dark: true,
            ui_font: UiFont {
                family: None,
                size: clamp_ui_font_size(Settings::default().ui_font_size),
            },
            problems: Vec::new(),
            generation: 0,
        };
        config.derive();
        config
    }

    /// Everything under `paths`, with the bundled value for whatever fails to load.
    pub fn load(paths: &ConfigPaths, system_dark: bool) -> Self {
        let mut config = Self::bundled(paths.clone());
        config.system_dark = system_dark;
        // Themes first, so the settings resolve their theme against the user's files
        // rather than warning that it is missing from the bundled ones.
        config.reload_themes();
        config.reload_keymap();
        config.reload_settings();
        config
    }

    pub fn paths(&self) -> &ConfigPaths {
        &self.paths
    }

    pub fn settings(&self) -> &Arc<Settings> {
        &self.settings
    }

    pub fn keymap(&self) -> &Arc<Keymap> {
        &self.keymap
    }

    /// The resolved Zed theme for the current appearance.
    pub fn theme(&self) -> &Arc<ZedTheme> {
        &self.theme
    }

    pub fn themes(&self) -> &Arc<ThemeRegistry> {
        &self.themes
    }

    pub fn ui_font(&self) -> &UiFont {
        &self.ui_font
    }

    pub fn is_dark(&self) -> bool {
        self.theme.is_dark()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// A Zed theme key with no gpui-kit counterpart, such as `toolbar.background`, for
    /// views that paint it themselves.
    pub fn color(&self, key: &str) -> Option<Hsla> {
        self.theme.color(key).map(to_hsla)
    }

    pub fn problems(&self) -> &[ConfigProblem] {
        &self.problems
    }

    /// One short line for a status bar: the first problem, with a count of the others. An
    /// error (a file that did not load) outranks a warning.
    pub fn notice(&self) -> Option<String> {
        let first = self
            .problems
            .iter()
            .find(|problem| problem.is_error)
            .or_else(|| self.problems.first())?;
        let more = self.problems.len() - 1;
        Some(if more == 0 {
            first.message.clone()
        } else {
            format!("{} (+{more} more)", first.message)
        })
    }

    fn set_problems(&mut self, piece: ConfigPiece, problems: Vec<ConfigProblem>) {
        self.problems.retain(|problem| problem.piece != piece);
        for problem in &problems {
            if problem.is_error {
                tracing::error!(%piece, "{}", problem.message);
            } else {
                tracing::warn!(%piece, "{}", problem.message);
            }
        }
        self.problems.extend(problems);
    }

    fn problem(
        &self,
        piece: ConfigPiece,
        file: &Path,
        message: String,
        is_error: bool,
    ) -> ConfigProblem {
        ConfigProblem {
            piece,
            file: file.to_path_buf(),
            message: format!("{}: {message}", short_name(file)),
            is_error,
        }
    }

    /// Read the settings files again. On failure the settings in use stay.
    pub fn reload_settings(&mut self) {
        let loaded = load_files::<Settings>(
            DEFAULT_SETTINGS,
            Some(&self.paths.settings),
            self.paths.project_settings.as_deref(),
        );
        let problems = match loaded {
            Ok(loaded) => {
                let warnings = loaded
                    .warnings
                    .iter()
                    .map(|warning| {
                        self.problem(
                            ConfigPiece::Settings,
                            &warning.file,
                            warning.message.clone(),
                            false,
                        )
                    })
                    .collect();
                self.settings = Arc::new(loaded.value);
                warnings
            }
            Err(error) => {
                let file = error.file().unwrap_or(&self.paths.settings).to_path_buf();
                vec![self.problem(
                    ConfigPiece::Settings,
                    &file,
                    format!("not applied: {}", settings_error_text(&error)),
                    true,
                )]
            }
        };
        self.set_problems(ConfigPiece::Settings, problems);
        self.derive();
    }

    /// Read the keymap file again over the bundled bindings. On failure the bindings in
    /// use stay.
    pub fn reload_keymap(&mut self) {
        let problems = match load_keymap(
            bundled_keymap_text(Platform::current()),
            Some(&self.paths.keymap),
        ) {
            Ok(keymap) => {
                self.keymap = Arc::new(keymap);
                Vec::new()
            }
            Err(error) => vec![self.problem(
                ConfigPiece::Keymap,
                &self.paths.keymap,
                format!("not applied: {}", keymap_error_text(&error)),
                true,
            )],
        };
        self.set_problems(ConfigPiece::Keymap, problems);
    }

    /// Read the themes folder again.
    pub fn reload_themes(&mut self) {
        match ThemeRegistry::load_from(BUNDLED_THEMES, THEME_DEFAULTS, &self.paths) {
            Ok(themes) => self.themes = Arc::new(themes),
            // The bundled themes are checked by a test; keep what is in use regardless.
            Err(error) => tracing::error!(%error, "bundled themes do not load"),
        }
        let folder = self.paths.themes.clone();
        let problems = self
            .themes
            .take_warnings()
            .into_iter()
            .map(|warning| {
                let file = Path::new(&warning.source);
                let file = if file.is_absolute() {
                    file
                } else {
                    folder.as_path()
                };
                self.problem(ConfigPiece::Themes, file, warning.message.clone(), false)
            })
            .collect();
        self.set_problems(ConfigPiece::Themes, problems);
        self.derive();
    }

    /// Follow a new window appearance. Returns whether the theme changed.
    pub fn set_system_dark(&mut self, dark: bool) -> bool {
        if self.system_dark == dark {
            return false;
        }
        self.system_dark = dark;
        let before = self.theme.name.clone();
        self.derive();
        self.theme.name != before
    }

    /// Recompute the theme and font from the settings and the registry.
    fn derive(&mut self) {
        let theme = self
            .themes
            .resolve(&self.settings.theme, self.system_dark)
            .clone();
        // A theme the settings name but no file provides comes back as a warning.
        let settings_file = self.paths.settings.clone();
        let missing = self
            .themes
            .take_warnings()
            .into_iter()
            .map(|warning| self.problem(ConfigPiece::Theme, &settings_file, warning.message, false))
            .collect();
        self.set_problems(ConfigPiece::Theme, missing);
        self.theme = Arc::new(theme);
        self.ui_font = UiFont {
            family: self
                .settings
                .ui_font_family
                .as_ref()
                .map(|family| family.trim().to_owned())
                .filter(|family| !family.is_empty()),
            size: clamp_ui_font_size(self.settings.ui_font_size),
        };
        // The font was derived afresh; `install` checks it against the installed families.
        self.set_problems(ConfigPiece::Fonts, Vec::new());
    }

    /// Swap a UI family the settings name but the machine lacks for the system UI font,
    /// and say so. GPUI would otherwise draw a missing family in its own fallback stack.
    pub fn substitute_missing_fonts(&mut self, installed: &[String]) {
        let mut problems = Vec::new();
        if let Some(family) = self.ui_font.family.clone()
            && missing_family(&family, installed)
        {
            self.ui_font.family = None;
            problems.push(self.problem(
                ConfigPiece::Fonts,
                &self.paths.settings,
                format!("font {family:?} is not installed; the system UI font is used"),
                false,
            ));
        }
        self.set_problems(ConfigPiece::Fonts, problems);
    }

    /// gpui-kit's theme for this configuration.
    pub fn kit_theme(&self) -> ThemeConfig {
        let theme = self.theme.clone();
        let lookup = move |key: &str| theme.color(key).map(to_hsla);
        kit_theme_config(
            &ZedColors {
                name: &self.theme.name,
                dark: self.theme.is_dark(),
                lookup: &lookup,
            },
            &self.ui_font,
        )
    }
}

fn bundled_themes() -> ThemeRegistry {
    ThemeRegistry::with_bundled(BUNDLED_THEMES, THEME_DEFAULTS).expect("the bundled themes parse")
}

/// A file as the status bar names it.
fn short_name(file: &Path) -> String {
    file.file_name().map_or_else(
        || file.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// The error without its absolute path, which the problem's own prefix replaces.
fn settings_error_text(error: &SettingsError) -> String {
    match error {
        SettingsError::Invalid {
            line,
            column,
            message,
            ..
        } => format!("{line}:{column}: {message}"),
        SettingsError::Io { source, .. } => source.to_string(),
        SettingsError::Merged(message) => message.clone(),
    }
}

fn keymap_error_text(error: &appconfig::KeymapError) -> String {
    match error {
        appconfig::KeymapError::Invalid {
            line,
            column,
            message,
            ..
        } => format!("{line}:{column}: {message}"),
        appconfig::KeymapError::Io { source, .. } => source.to_string(),
    }
}

/// Whether `family` is one `installed` does not list. Families GPUI provides itself
/// (`.SystemUIFont`) are never missing, and an empty list (a text system that cannot
/// list fonts) changes nothing.
pub fn missing_family(family: &str, installed: &[String]) -> bool {
    !(installed.is_empty()
        || family.starts_with('.')
        || installed
            .iter()
            .any(|name| name.eq_ignore_ascii_case(family)))
}

/// The font families installed on this machine, listed once per process: listing costs
/// around a hundred milliseconds on macOS, so it only happens when settings name a family.
fn installed_families(cx: &App) -> &'static [String] {
    static NAMES: OnceLock<Vec<String>> = OnceLock::new();
    NAMES.get_or_init(|| cx.text_system().all_font_names())
}

/// Install `config`: gpui-kit's theme and UI font, the key bindings (when the keymap
/// changed), then the global, which notifies every observer.
pub fn install(mut config: Config, cx: &mut App) {
    let previous = cx.try_global::<Config>();
    config.generation = previous.map_or(1, |previous| previous.generation + 1);
    let rebind = previous.is_none_or(|previous| !Arc::ptr_eq(&previous.keymap, &config.keymap));
    if config.ui_font.family.is_some() {
        let installed = installed_families(cx);
        config.substitute_missing_fonts(installed);
    }
    apply_kit_theme(config.kit_theme(), cx);
    if rebind {
        let file = config.paths.keymap.clone();
        let problems = keymap::apply(&config.keymap, cx)
            .into_iter()
            .map(|message| ConfigProblem {
                piece: ConfigPiece::Bindings,
                file: file.clone(),
                message,
                is_error: false,
            })
            .collect();
        config.set_problems(ConfigPiece::Bindings, problems);
    }
    cx.set_global(config);
}

/// Change the installed configuration with `edit`, then install the result. Does nothing
/// before the first install: there is nothing to change.
fn update(cx: &mut App, edit: impl FnOnce(&mut Config)) {
    let Some(mut config) = cx.try_global::<Config>().cloned() else {
        return;
    };
    edit(&mut config);
    install(config, cx);
}

/// Reload one piece from disk and install the result.
pub fn reload(piece: ConfigPiece, cx: &mut App) {
    tracing::info!(%piece, "reloading configuration");
    update(cx, |config| match piece {
        ConfigPiece::Settings | ConfigPiece::Fonts => config.reload_settings(),
        ConfigPiece::Keymap | ConfigPiece::Bindings => config.reload_keymap(),
        ConfigPiece::Themes | ConfigPiece::Theme => config.reload_themes(),
    });
}

/// Follow a window appearance change: re-resolves a `"mode": "system"` theme.
pub fn set_appearance(system_dark: bool, cx: &mut App) {
    let changed = cx
        .try_global::<Config>()
        .is_some_and(|config| config.system_dark != system_dark);
    if changed {
        update(cx, |config| {
            config.set_system_dark(system_dark);
        });
    }
}

/// Where the config lives: `config_dir` (the `--config-dir` flag) if given, else
/// `CROSSFADE_CONFIG_DIR`, else the platform directory. A project
/// `.crossfade/settings.json` above the working directory joins it.
pub fn paths_for(config_dir: Option<PathBuf>) -> ConfigPaths {
    let paths = match config_dir {
        Some(dir) => ConfigPaths::new(dir),
        None => ConfigPaths::default_for_platform(&IDENTITY),
    };
    match std::env::current_dir() {
        Ok(cwd) => paths.with_project_from(&IDENTITY, &cwd),
        Err(_) => paths,
    }
}

/// Make sure `settings.json` exists (created from the commented template, never
/// overwritten) and return its path, for the caller to open in the user's editor with
/// `cx.open_with_system(&path)`.
pub fn open_settings_file(paths: &ConfigPaths) -> io::Result<PathBuf> {
    paths.ensure_settings_file(SETTINGS_TEMPLATE)?;
    Ok(paths.settings.clone())
}

/// The same for `keymap.json`.
pub fn open_keymap_file(paths: &ConfigPaths) -> io::Result<PathBuf> {
    paths.ensure_keymap_file(KEYMAP_TEMPLATE)?;
    Ok(paths.keymap.clone())
}

/// Longest the watcher task blocks on the channel (on the background executor) before
/// looking again, so it notices the app going away.
const WATCH_POLL: Duration = Duration::from_millis(50);

/// Pause between batches of watcher events.
const WATCH_FRAME: Duration = Duration::from_millis(8);

/// The piece a watcher event asks to reload; `None` for folders this app does not watch.
fn piece_of(event: ConfigEvent) -> Option<ConfigPiece> {
    match event {
        ConfigEvent::Settings => Some(ConfigPiece::Settings),
        ConfigEvent::Keymap => Some(ConfigPiece::Keymap),
        ConfigEvent::Themes => Some(ConfigPiece::Themes),
        ConfigEvent::Extra(_) => None,
    }
}

/// The running watcher and the task draining it, kept alive as a global.
struct Watching {
    _watcher: ConfigWatcher,
    _task: Task<()>,
}

impl Global for Watching {}

/// Reload whatever the files under `paths` change, until the app quits or `watch` is
/// called again. The watcher's thread only sends events; loads and installs run on the
/// main thread, once per piece that changed in each batch.
fn watch(paths: &ConfigPaths, cx: &mut App) {
    let (tx, rx) = crossbeam_channel::unbounded::<ConfigEvent>();
    let watcher = ConfigWatcher::spawn(paths, &[], tx);
    let task = cx.spawn(async move |cx| {
        loop {
            let events = rx.clone();
            let (batch, closed) = cx
                .background_spawn(async move {
                    match events.recv_timeout(WATCH_POLL) {
                        Ok(first) => {
                            let mut batch = vec![first];
                            batch.extend(events.try_iter());
                            (batch, false)
                        }
                        Err(RecvTimeoutError::Timeout) => (Vec::new(), false),
                        Err(RecvTimeoutError::Disconnected) => (Vec::new(), true),
                    }
                })
                .await;
            let mut pieces: Vec<ConfigPiece> = Vec::new();
            for piece in batch.into_iter().filter_map(piece_of) {
                if !pieces.contains(&piece) {
                    pieces.push(piece);
                }
            }
            if !pieces.is_empty() {
                cx.update(|cx| {
                    for piece in pieces {
                        reload(piece, cx);
                    }
                });
            }
            if closed {
                break;
            }
            cx.background_executor().timer(WATCH_FRAME).await;
        }
    });
    cx.set_global(Watching {
        _watcher: watcher,
        _task: task,
    });
}

/// Load and install the configuration under `paths`, then watch it for changes.
pub fn start(paths: ConfigPaths, cx: &mut App) {
    let dark = matches!(
        cx.window_appearance(),
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    );
    tracing::info!(dir = %paths.dir.display(), "loading configuration");
    install(Config::load(&paths, dark), cx);
    watch(&paths, cx);
}
