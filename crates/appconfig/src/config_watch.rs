//! Hot reload: watches the config directory and reports which kind of file changed.
//!
//! The watcher observes the config *directory*, not the files in it, so a settings file
//! created after the app started is noticed, and so is one an editor replaces by writing
//! a temp file and renaming it. Changes are debounced for 100 ms and coalesced: however
//! many files change in one window, each [`ConfigEvent`] is sent once per batch.
//!
//! Files produce events; directories mostly do not. A directory event, including the
//! creation of the config, `themes/` or an app-declared folder that
//! [`ConfigWatcher::spawn`] performs itself, is dropped: some backends (FSEvents on
//! macOS) deliver directory events late and folded into later batches, and a reload
//! for one would be spurious. The two exceptions, folders inside a recursive
//! [`WatchSpec`] folder and the watched folders themselves coming and going, are below. A
//! batch may still be split across debounce windows, so one save can produce more than
//! one event of a kind; treat an event as "reload this", never as "exactly one change
//! happened".
//!
//! The event says what to reload, not what changed; the receiver re-reads the file with
//! [`load_files`](crate::settings::load_files), [`load_keymap`](crate::keymap::load_keymap)
//! or [`ThemeRegistry::load`](crate::theme::ThemeRegistry::load), or lists the extra
//! folder again.
//!
//! # Folders, and why Linux needs more
//!
//! `themes/` is watched recursively, and each [`WatchSpec`] folder recursively or flat as
//! it asks, each with a watch of its own; `spawn` creates them all first so they can be.
//! The backends differ in two ways that matter:
//!
//! - **New subfolders.** Linux's inotify watches one directory at a time; notify
//!   emulates a recursive watch by adding a watch for each subfolder when it sees the
//!   subfolder appear. A file written into a new subfolder before that watch is in place
//!   is never reported, so `mkdir -p lib && cp util.lua lib/` loses the file's event.
//!   Inside a recursive extra folder a folder appearing, disappearing or being renamed
//!   therefore counts as that folder's [`ConfigEvent::Extra`] by itself, and the
//!   receiver's relist finds whatever the folder holds by then (the debounce gives it
//!   100 ms). FSEvents (macOS) and ReadDirectoryChangesW (Windows) watch whole trees
//!   and report the file too; for them the folder event is one more harmless relist.
//! - **Folders that go and come back.** On Linux (and with kqueue) and Windows a watch
//!   belongs to the directory, not the path: when `themes/` or an extra folder is
//!   deleted or renamed away its watch dies with it (or, on Windows, follows the
//!   renamed folder), and the config directory's flat watch only sees a new folder of
//!   that name appear. So when a batch touches one of them, the forwarding thread looks
//!   at the disk: a folder that is gone loses its watch, and one that is there but
//!   unwatched, or was removed or renamed in the batch, gets its old watch dropped and a
//!   new one added, so a path is never watched twice. Either way the folder's event is
//!   reported, because files may have arrived before the new watch did. A folder that
//!   is still watched and was not removed is left alone, which also ignores the late
//!   events FSEvents delivers for the folders `spawn` made.
//!
//!   FSEvents (macOS) and the polling backend watch paths, so a folder that comes back
//!   is watched already, and the watcher never drops or adds watches there: FSEvents
//!   folds a quick delete and re-create of one path into flags the debouncer may cancel
//!   out, and a re-create it never saw must not leave the path unwatched. It still
//!   reports a folder going or coming back when it sees one.
//!
//! Making a watched folder while the app runs cannot race the watcher: `create_dir_all`
//! accepts a folder another thread made first, and a folder made after its watch died is
//! re-watched and reported as above.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, SendTimeoutError, Sender, select, unbounded};
use notify::event::{CreateKind, ModifyKind, RemoveKind, RenameMode};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher, WatcherKind};
use notify_debouncer_full::{
    DebounceEventResult, DebouncedEvent, Debouncer, RecommendedCache, new_debouncer,
};
use parking_lot::Mutex;

use crate::paths::ConfigPaths;

/// How long changes are gathered before they are reported.
pub const DEBOUNCE: Duration = Duration::from_millis(100);

/// How long a send waits on a full channel before giving up on that event.
const SEND_TIMEOUT: Duration = Duration::from_millis(100);

/// Which kind of configuration changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ConfigEvent {
    /// `settings.json` in the config directory, or the project settings file.
    Settings,
    /// `keymap.json`.
    Keymap,
    /// A `*.json` file under the `themes/` folder, at any depth.
    Themes,
    /// A change in the app-declared folder at this index of the [`WatchSpec`] slice
    /// given to [`ConfigWatcher::spawn`].
    Extra(usize),
}

/// An app-declared folder to watch, such as `scripts/` or `plugins/`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchSpec {
    /// The folder. Made if it is missing.
    pub path: PathBuf,
    /// Watch the whole tree, in which a folder appearing, disappearing or being renamed
    /// also counts (see the module docs); otherwise only the files directly in it, and
    /// folders never count.
    pub recursive: bool,
    /// The file extensions that count, without the dot and compared without regard to
    /// case. Empty means any file.
    pub extensions: Vec<String>,
}

impl WatchSpec {
    pub fn new(path: impl Into<PathBuf>, recursive: bool, extensions: &[&str]) -> Self {
        Self {
            path: path.into(),
            recursive,
            extensions: extensions.iter().map(|ext| (*ext).to_string()).collect(),
        }
    }
}

/// A [`WatchSpec`] in the form the OS reports paths.
#[derive(Clone, Debug)]
struct Extra {
    path: PathBuf,
    recursive: bool,
    extensions: Vec<String>,
}

impl Extra {
    #[cfg(test)]
    fn new(path: impl Into<PathBuf>, recursive: bool, extensions: &[&str]) -> Self {
        Self {
            path: path.into(),
            recursive,
            extensions: extensions.iter().map(|ext| (*ext).to_string()).collect(),
        }
    }

    fn from_spec(spec: &WatchSpec) -> Self {
        Self {
            path: canonical(&spec.path),
            recursive: spec.recursive,
            extensions: spec.extensions.clone(),
        }
    }

    fn wants(&self, path: &Path) -> bool {
        self.extensions.is_empty()
            || path.extension().is_some_and(|ext| {
                self.extensions
                    .iter()
                    .any(|wanted| ext.eq_ignore_ascii_case(wanted))
            })
    }
}

/// The files the watcher cares about, in the form the OS reports them.
#[derive(Clone, Debug)]
struct Targets {
    settings: Vec<PathBuf>,
    keymap: PathBuf,
    themes: PathBuf,
    extras: Vec<Extra>,
}

impl Targets {
    fn new(paths: &ConfigPaths, extras: &[WatchSpec]) -> Self {
        let mut settings = vec![canonical(&paths.settings)];
        settings.extend(paths.project_settings.iter().map(|path| canonical(path)));
        Self {
            settings,
            keymap: canonical(&paths.keymap),
            themes: canonical(&paths.themes),
            extras: extras.iter().map(Extra::from_spec).collect(),
        }
    }

    /// The kind of config file `path` names, if it names one. Directories never do,
    /// and neither does the `themes/` folder itself.
    fn classify(&self, path: &Path) -> Option<ConfigEvent> {
        let kind = if self.settings.iter().any(|settings| settings == path) {
            ConfigEvent::Settings
        } else if path == self.keymap {
            ConfigEvent::Keymap
        } else if path != self.themes
            && path.starts_with(&self.themes)
            && path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        {
            ConfigEvent::Themes
        } else {
            let index = self.extras.iter().position(|extra| {
                !extra.recursive && path.parent() == Some(extra.path.as_path()) && extra.wants(path)
            })?;
            ConfigEvent::Extra(index)
        };
        // A directory that happens to carry a config file's name is not one. A file
        // that was just deleted is not a directory either, so removals still count.
        (!path.is_dir()).then_some(kind)
    }

    /// The recursive extra folder an event of `kind` on `path` changes, if any: a file
    /// the spec wants at any depth, or a folder inside it that appears, disappears or
    /// is renamed.
    fn changes_extra(&self, kind: &EventKind, path: &Path) -> Option<ConfigEvent> {
        self.extras
            .iter()
            .position(|extra| extra.recursive && changes_tree(extra, kind, path))
            .map(ConfigEvent::Extra)
    }

    /// The distinct events for a batch, in a fixed order.
    fn events(&self, batch: &[DebouncedEvent]) -> Vec<ConfigEvent> {
        let mut found = Vec::new();
        for event in batch {
            if matches!(event.kind, EventKind::Access(_)) {
                continue;
            }
            let folder_event = matches!(
                event.kind,
                EventKind::Create(CreateKind::Folder) | EventKind::Remove(RemoveKind::Folder)
            );
            for path in &event.paths {
                let kind = if let Some(extra) = self.changes_extra(&event.kind, path) {
                    Some(extra)
                } else if folder_event {
                    None
                } else {
                    self.classify(path)
                };
                if let Some(kind) = kind
                    && !found.contains(&kind)
                {
                    found.push(kind);
                }
            }
        }
        found.sort_unstable();
        found
    }
}

/// Whether an event of `kind` on `path` changes the tree of a recursive extra folder: a
/// file it wants at any depth, or a folder inside it that appears, disappears or is
/// renamed, which on Linux may hold files whose own events were never delivered (see the
/// module docs). Anything removed or renamed in there counts, since what is gone cannot
/// be told apart from a folder; a spurious reload is cheap.
fn changes_tree(extra: &Extra, kind: &EventKind, path: &Path) -> bool {
    if path == extra.path || !path.starts_with(&extra.path) {
        return false;
    }
    // With no extensions listed every file is wanted, but the event kinds below still
    // tell a file's change from a folder's.
    let wanted = extra.wants(path);
    match kind {
        EventKind::Access(_) => false,
        EventKind::Create(CreateKind::Folder)
        | EventKind::Remove(_)
        | EventKind::Modify(ModifyKind::Name(_)) => true,
        // Windows reports `Create(Any)` for folders.
        EventKind::Create(_) => path.is_dir() || wanted,
        _ => wanted && !path.is_dir(),
    }
}

type SharedDebouncer = Arc<Mutex<Option<Debouncer<RecommendedWatcher, RecommendedCache>>>>;

/// A folder with a watch of its own: `themes/` or an app-declared one.
#[derive(Clone, Debug)]
struct OwnFolder {
    /// As the OS reports it.
    path: PathBuf,
    mode: RecursiveMode,
    /// What to report when it comes back or goes away.
    event: ConfigEvent,
    /// It was there, and watched, when last seen.
    present: bool,
    /// The backend holds a watch for its path.
    watched: bool,
}

/// The own folders and the debouncer that watches them, so that a folder deleted and
/// made again, or renamed into place, gets its watch back (see the module docs).
#[derive(Default)]
struct OwnFolders {
    folders: Vec<OwnFolder>,
    debouncer: SharedDebouncer,
    /// The backend's watches belong to directories (inotify, kqueue, Windows), so a
    /// folder that comes back needs a new one. Path-based backends (FSEvents, polling)
    /// keep watching the path.
    rewatch: bool,
}

/// Whether watches of the backend in use belong to a directory rather than its path.
fn watches_follow_directories() -> bool {
    !matches!(
        RecommendedWatcher::kind(),
        WatcherKind::Fsevent | WatcherKind::PollWatcher | WatcherKind::NullWatcher
    )
}

/// What to do about an own folder a batch touched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FolderAction {
    /// Drop any watch on the path and watch the folder there now.
    Rewatch,
    /// The folder is gone: drop its watch.
    Unwatch,
    /// Still watched and never removed (a late create event, say): leave it.
    Keep,
}

/// What to do about an own folder that is (`exists`) or is not on disk now, that the
/// batch `removed` or renamed away at some point, and that was `present` (there and
/// watched) before.
fn folder_action(exists: bool, removed: bool, present: bool) -> FolderAction {
    match (exists, present) {
        (true, false) => FolderAction::Rewatch,
        (true, true) if removed => FolderAction::Rewatch,
        (false, true) => FolderAction::Unwatch,
        _ => FolderAction::Keep,
    }
}

/// Which of `folders` `batch` touched, by index, each with whether it was removed or
/// renamed away at some point in the batch. Only events on a folder's own path count,
/// of any kind but access: FSEvents may report a folder deleted soon after it was made
/// as nothing but a metadata change, so the caller decides by what is on disk.
fn folder_changes(folders: &[PathBuf], batch: &[DebouncedEvent]) -> Vec<(usize, bool)> {
    let mut changes: Vec<(usize, bool)> = Vec::new();
    for event in batch {
        for (position, path) in event.paths.iter().enumerate() {
            let removed = match event.kind {
                EventKind::Access(_) => continue,
                EventKind::Remove(_) => true,
                EventKind::Modify(ModifyKind::Name(mode)) => match mode {
                    RenameMode::To => false,
                    // The first path is where it came from.
                    RenameMode::Both => position == 0,
                    // A rename with no direction (FSEvents): re-watch if it is there.
                    _ => true,
                },
                _ => false,
            };
            let Some(index) = folders.iter().position(|folder| folder == path) else {
                continue;
            };
            match changes.iter_mut().find(|(seen, _)| *seen == index) {
                Some((_, was_removed)) => *was_removed |= removed,
                None => changes.push((index, removed)),
            }
        }
    }
    changes
}

impl OwnFolders {
    /// Bring the watches in line with what `batch` did to the own folders. Returns the
    /// events of the folders that came back or went away.
    fn update(&mut self, batch: &[DebouncedEvent]) -> Vec<ConfigEvent> {
        let paths: Vec<PathBuf> = self.folders.iter().map(|f| f.path.clone()).collect();
        let mut events = Vec::new();
        for (index, removed) in folder_changes(&paths, batch) {
            let folder = &mut self.folders[index];
            let action = folder_action(folder.path.is_dir(), removed, folder.present);
            if action == FolderAction::Keep {
                continue;
            }
            folder.present = action == FolderAction::Rewatch;
            events.push(folder.event);
            // A path-based watch outlives the folder: only a path never watched (it
            // could not be at spawn) needs one.
            let (drop_old, add_new) = if self.rewatch {
                (true, action == FolderAction::Rewatch)
            } else {
                (false, action == FolderAction::Rewatch && !folder.watched)
            };
            if !drop_old && !add_new {
                continue;
            }
            let mut debouncer = self.debouncer.lock();
            // The watcher is being dropped.
            let Some(debouncer) = debouncer.as_mut() else {
                break;
            };
            if drop_old {
                // Whatever watch the path still has belongs to the old folder (or is
                // gone already, which is an error to ignore): drop it before adding a
                // new one, so the path is never watched twice.
                let _ = debouncer.unwatch(&folder.path);
                folder.watched = false;
            }
            if add_new {
                match debouncer.watch(&folder.path, folder.mode) {
                    Ok(()) => {
                        folder.watched = true;
                        tracing::debug!(dir = %folder.path.display(), "watching the folder again");
                    }
                    Err(err) => {
                        folder.present = false;
                        tracing::warn!(%err, dir = %folder.path.display(), "cannot watch the folder");
                    }
                }
            }
        }
        events
    }
}

/// `path` as the OS will report it: symlinks resolved, and on macOS `/var` spelled
/// `/private/var`. A path that does not exist yet is resolved through its parent.
fn canonical(path: &Path) -> PathBuf {
    if let Ok(resolved) = std::fs::canonicalize(path) {
        return resolved;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => std::fs::canonicalize(parent)
            .map(|parent| parent.join(name))
            .unwrap_or_else(|_| path.to_path_buf()),
        _ => path.to_path_buf(),
    }
}

/// Watches the config directory until dropped.
///
/// Two threads run behind it: the debouncer's, which only queues batches, and a
/// forwarding thread that classifies them and sends [`ConfigEvent`]s. The forwarding
/// thread owns the caller's sender.
pub struct ConfigWatcher {
    /// Shared with the forwarding thread, which re-watches folders that come back.
    /// `None` once stopped, or if the OS watcher never started.
    debouncer: SharedDebouncer,
    /// Dropping this wakes the forwarding thread and tells it to stop.
    stop: Option<Sender<()>>,
    forwarder: Option<JoinHandle<()>>,
    alive: Arc<AtomicBool>,
}

impl ConfigWatcher {
    /// Starts watching and sends a [`ConfigEvent`] on `tx` for each change.
    ///
    /// Starts watching and sends a [`ConfigEvent`] on `tx` for each change.
    ///
    /// Watches the config directory non-recursively, `themes/` recursively and each of
    /// `extras` as its spec says (`ConfigEvent::Extra(i)` is `extras[i]`), creating each
    /// folder if it is missing so files added later are noticed, plus the directory of
    /// the project settings file when `paths` has one. Creating those directories
    /// produces no event. If the OS watcher cannot be started the problem is logged and
    /// the returned watcher is inert ([`is_active`](Self::is_active) is false); the app
    /// still runs, just without hot reload.
    ///
    /// Dropping the watcher is synchronous: it stops the OS watcher, wakes and joins the
    /// forwarding thread, and so drops `tx`. No event is sent after `drop` returns, and
    /// the receiver sees the channel disconnect once it has read what was already sent.
    pub fn spawn(
        paths: &ConfigPaths,
        extras: &[WatchSpec],
        tx: Sender<ConfigEvent>,
    ) -> ConfigWatcher {
        match start(paths, extras, tx) {
            Ok(watcher) => watcher,
            Err(err) => {
                tracing::warn!(%err, dir = %paths.dir.display(), "config hot reload is off");
                ConfigWatcher {
                    debouncer: SharedDebouncer::default(),
                    stop: None,
                    forwarder: None,
                    alive: Arc::new(AtomicBool::new(false)),
                }
            }
        }
    }

    /// Whether the OS watcher is running.
    pub fn is_active(&self) -> bool {
        self.debouncer.lock().is_some()
    }
}

impl Drop for ConfigWatcher {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::SeqCst);
        // Taken out first: the forwarder may be re-watching a folder under the lock.
        let debouncer = self.debouncer.lock().take();
        if let Some(debouncer) = debouncer {
            // Waits for the debouncer's thread, at most one tick (a quarter of the
            // debounce). After this nothing new reaches the forwarder.
            debouncer.stop();
        }
        // Wake the forwarder out of its wait, then wait for it. It owns the caller's
        // sender, so once it has gone no event can be sent.
        drop(self.stop.take());
        if let Some(forwarder) = self.forwarder.take()
            && forwarder.join().is_err()
        {
            tracing::warn!("the config watcher's forwarding thread panicked");
        }
    }
}

impl std::fmt::Debug for ConfigWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("ConfigWatcher")
            .field("active", &self.is_active())
            .finish()
    }
}

type Batch = Vec<DebouncedEvent>;

/// The forwarding thread: turns raw batches into events, and re-watches own folders
/// that come back, until told to stop.
fn forward(
    targets: &Targets,
    folders: &mut OwnFolders,
    batches: &Receiver<Batch>,
    stop: &Receiver<()>,
    tx: &Sender<ConfigEvent>,
    alive: &AtomicBool,
) {
    loop {
        select! {
            // Nothing is ever sent on `stop`; it fires when the watcher drops its end.
            recv(stop) -> _ => return,
            recv(batches) -> batch => {
                let Ok(batch) = batch else { return };
                let mut events = targets.events(&batch);
                for event in folders.update(&batch) {
                    if !events.contains(&event) {
                        events.push(event);
                    }
                }
                events.sort_unstable();
                for event in events {
                    // Checked per event so nothing is sent once the watcher is dropped.
                    if !alive.load(Ordering::SeqCst) {
                        return;
                    }
                    match tx.send_timeout(event, SEND_TIMEOUT) {
                        Ok(()) => {}
                        Err(SendTimeoutError::Timeout(_)) => {
                            tracing::debug!(?event, "config event not delivered: channel full");
                        }
                        // Nobody is listening any more.
                        Err(SendTimeoutError::Disconnected(_)) => return,
                    }
                }
            }
        }
    }
}

fn start(
    paths: &ConfigPaths,
    extras: &[WatchSpec],
    tx: Sender<ConfigEvent>,
) -> Result<ConfigWatcher, notify::Error> {
    // The directories have to exist to be watched.
    for dir in [&paths.dir, &paths.themes]
        .into_iter()
        .chain(extras.iter().map(|spec| &spec.path))
    {
        if let Err(err) = std::fs::create_dir_all(dir) {
            tracing::warn!(%err, dir = %dir.display(), "cannot create the config directory");
        }
    }
    let targets = Targets::new(paths, extras);

    let (batch_tx, batch_rx) = unbounded::<Batch>();
    let mut debouncer = new_debouncer(DEBOUNCE, None, move |result: DebounceEventResult| {
        match result {
            // The forwarder may already be gone, which is fine.
            Ok(batch) => drop(batch_tx.send(batch)),
            Err(errors) => {
                for err in errors {
                    tracing::warn!(%err, "config watcher error");
                }
            }
        }
    })?;

    let config_dir = canonical(&paths.dir);
    debouncer.watch(&config_dir, RecursiveMode::NonRecursive)?;
    let mut own = Vec::new();
    let folders = std::iter::once((
        canonical(&paths.themes),
        RecursiveMode::Recursive,
        ConfigEvent::Themes,
    ))
    .chain(targets.extras.iter().enumerate().map(|(index, extra)| {
        let mode = if extra.recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        (extra.path.clone(), mode, ConfigEvent::Extra(index))
    }));
    for (path, mode, event) in folders {
        if path == config_dir {
            continue;
        }
        // A folder that could not be watched now is watched once it appears.
        let watched = match debouncer.watch(&path, mode) {
            Ok(()) => true,
            Err(err) => {
                tracing::warn!(%err, dir = %path.display(), "cannot watch the folder");
                false
            }
        };
        own.push(OwnFolder {
            path,
            mode,
            event,
            present: watched,
            watched,
        });
    }
    // The project file usually lives outside the config directory.
    let mut watched = vec![config_dir.clone()];
    watched.extend(own.iter().map(|folder| folder.path.clone()));
    if let Some(parent) = paths
        .project_settings
        .as_deref()
        .and_then(Path::parent)
        .map(canonical)
        && !watched.contains(&parent)
        && let Err(err) = debouncer.watch(&parent, RecursiveMode::NonRecursive)
    {
        tracing::warn!(%err, dir = %parent.display(), "cannot watch the project settings");
    }

    let debouncer: SharedDebouncer = Arc::new(Mutex::new(Some(debouncer)));
    let mut folders = OwnFolders {
        folders: own,
        debouncer: Arc::clone(&debouncer),
        rewatch: watches_follow_directories(),
    };
    let alive = Arc::new(AtomicBool::new(true));
    let (stop_tx, stop_rx) = unbounded::<()>();
    let forwarder = {
        let alive = Arc::clone(&alive);
        std::thread::Builder::new()
            .name("appconfig-watch".to_string())
            .spawn(move || forward(&targets, &mut folders, &batch_rx, &stop_rx, &tx, &alive))
            .map_err(notify::Error::io)?
    };
    Ok(ConfigWatcher {
        debouncer,
        stop: Some(stop_tx),
        forwarder: Some(forwarder),
        alive,
    })
}

#[cfg(test)]
mod tests;
