//! Headless-window helpers shared by the UI tests.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::config::{self, Config};
use crate::config_test_support::TempDir as ConfigDir;
use crate::prelude::*;
use crate::scripted::Script;
use crate::workspace::{Workspace, WorkspaceOptions, init};

/// A laptop-sized test window.
pub(crate) const TEST_WINDOW: (f32, f32) = (1180., 760.);

/// About a frame at 120 Hz.
pub(crate) const FRAME: Duration = Duration::from_millis(8);

/// A scratch directory, removed on drop.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub(crate) fn new(tag: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "crossfade-ui-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the scratch directory");
        Self(dir)
    }

    pub(crate) fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }

    pub(crate) fn write(&self, name: &str, bytes: &[u8]) -> String {
        std::fs::write(self.0.join(name), bytes).expect("write a scratch file");
        self.path(name)
    }

    /// A 16-bit PCM WAV whose sample `i` is `i * 100` (wrapping), so converted output
    /// can be checked sample by sample.
    pub(crate) fn write_wav(&self, name: &str, rate: u32, channels: u16, frames: u32) -> String {
        self.write(name, &wav_s16(rate, channels, frames))
    }

    /// A mono 16-bit WAV that claims `frames` frames of silence, backed by a sparse file.
    pub(crate) fn write_sparse_wav(&self, name: &str, rate: u32, frames: u32) -> String {
        let path = self.path(name);
        let header = wav_s16(rate, 1, 0);
        let data_len = frames * 2;
        let mut bytes = header[..40].to_vec();
        bytes.extend_from_slice(&data_len.to_le_bytes());
        std::fs::write(&path, bytes).expect("write the header");
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("reopen");
        file.set_len(44 + u64::from(data_len)).expect("extend");
        path
    }

    pub(crate) fn listing(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.0)
            .expect("read the scratch directory")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn sample_at(index: u32) -> i16 {
    i16::try_from(i64::from(index) * 100 % 30_000).expect("in range")
}

pub(crate) fn wav_s16(rate: u32, channels: u16, frames: u32) -> Vec<u8> {
    let data_len = frames * u32::from(channels) * 2;
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
    v.extend_from_slice(&(channels * 2).to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..frames * u32::from(channels) {
        v.extend_from_slice(&sample_at(i).to_le_bytes());
    }
    v
}

/// The paths the app asked the platform to open, in order.
#[derive(Clone, Default)]
pub(crate) struct Opened(pub(crate) std::rc::Rc<std::cell::RefCell<Vec<PathBuf>>>);

impl Global for Opened {}

pub(crate) fn opened(cx: &mut TestAppContext) -> Vec<PathBuf> {
    cx.update(|cx| cx.global::<Opened>().0.borrow().clone())
}

/// A window over a scripted runner and a scratch directory.
pub(crate) struct Rig {
    pub(crate) window: AnyWindowHandle,
    pub(crate) workspace: Entity<Workspace>,
    pub(crate) script: Script,
    pub(crate) dir: TempDir,
    /// The config directory the window's `Config` was loaded from.
    pub(crate) config: ConfigDir,
}

pub(crate) fn open_rig(cx: &mut TestAppContext) -> Rig {
    open_rig_in(cx, ConfigDir::new("rig-config"))
}

/// A rig whose configuration is whatever `config` holds when the window opens.
pub(crate) fn open_rig_in(cx: &mut TestAppContext, config: ConfigDir) -> Rig {
    let (script, options) = Script::options();
    let (window, workspace) = open_in(cx, options, &config);
    Rig {
        window,
        workspace,
        script,
        dir: TempDir::new("rig"),
        config,
    }
}

/// Open a headless window holding a [`Workspace`] over `options`, with the app
/// initialised as `main` does.
pub(crate) fn open_with(
    cx: &mut TestAppContext,
    options: WorkspaceOptions,
) -> (AnyWindowHandle, Entity<Workspace>) {
    open_in(cx, options, &ConfigDir::new("window-config"))
}

/// [`open_with`] over the configuration in `config`: tests never read the real one.
pub(crate) fn open_in(
    cx: &mut TestAppContext,
    options: WorkspaceOptions,
    config: &ConfigDir,
) -> (AnyWindowHandle, Entity<Workspace>) {
    let paths = config.paths();
    cx.update(|cx| {
        // Animations run on the wall clock, not the test clock; with reduced motion
        // they show their end state from the first frame.
        cx.set_reduce_motion(true);
        init(cx);
        let log = Opened::default();
        let record = log.clone();
        cx.set_global(log);
        cx.set_global(config::Opener(std::rc::Rc::new(move |path, _| {
            record.0.borrow_mut().push(path.to_path_buf());
        })));
        config::install(Config::load(&paths, true), cx);
        let bounds = Bounds {
            origin: Point::default(),
            size: size(px(TEST_WINDOW.0), px(TEST_WINDOW.1)),
        };
        kit_open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| Workspace::new(options, window, cx)),
        )
        .expect("open test window")
    })
}

/// A window with nothing scripted, for tests of the chrome alone.
pub(crate) fn open_test_window(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
    let (_, options) = Script::options();
    open_with(cx, options)
}

/// Add `paths` through the workspace and let the probes finish.
pub(crate) fn add_paths(cx: &mut TestAppContext, rig: &Rig, paths: &[&str]) {
    let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    cx.update_window(rig.window, |_, window, cx| {
        rig.workspace
            .update(cx, |workspace, cx| workspace.add_paths(paths, window, cx));
    })
    .expect("the window is open");
    cx.run_until_parked();
    draw(cx, rig.window);
}

/// Whether the last frame has an element called `id`.
pub(crate) fn has(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    id: impl Into<ElementId>,
) -> bool {
    let id = id.into();
    cx.update_window(window, |_, window, _| window.try_find(id).is_some())
        .expect("the window is open")
}

/// Whether the element called `id` is disabled.
pub(crate) fn disabled(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    id: impl Into<ElementId>,
) -> bool {
    let id = id.into();
    cx.update_window(window, |_, window, _| {
        window.find(id).disabled().unwrap_or(false)
    })
    .expect("the window is open")
}

/// Draw the window twice, so a layout that depends on the last frame has settled.
pub(crate) fn draw(cx: &mut TestAppContext, window: AnyWindowHandle) {
    for _ in 0..2 {
        cx.update_window(window, |_, window, cx| window.render_frame(cx))
            .expect("the window is open");
        cx.run_until_parked();
    }
}

/// Advance `frames` frames, running whatever they wake.
pub(crate) fn step(cx: &mut TestAppContext, frames: usize) {
    for _ in 0..frames {
        cx.executor().advance_clock(FRAME);
        cx.run_until_parked();
    }
}

/// Click the element with `id`, as a user would.
pub(crate) fn click(cx: &mut TestAppContext, window: AnyWindowHandle, id: impl Into<ElementId>) {
    let id = id.into();
    cx.update_window(window, |_, window, cx| window.click(id, cx))
        .expect("the window is open");
    cx.run_until_parked();
}

/// Press `keys` in the window.
pub(crate) fn press(cx: &mut TestAppContext, window: AnyWindowHandle, keys: &str) {
    cx.update_window(window, |_, window, cx| window.press(keys, cx))
        .expect("the window is open");
    cx.run_until_parked();
}

pub(crate) fn open_sheet(cx: &mut TestAppContext, rig: &Rig) {
    click(cx, rig.window, "toolbar-convert");
    step(cx, 3);
    draw(cx, rig.window);
}

pub(crate) const PRESET_LABELS: [&str; 8] = [
    "Same as source",
    "16-bit (CD)",
    "24-bit",
    "32-bit float",
    "FLAC",
    "FLAC 16-bit",
    "FLAC 24-bit",
    "AIFF",
];
pub(crate) const CONTAINER_LABELS: [&str; 4] = ["WAV", "FLAC", "AIFF", "Ogg FLAC"];
pub(crate) const LEVEL_LABELS: [&str; 10] = [
    "Default",
    "0 \u{b7} Fastest",
    "1",
    "2",
    "3",
    "4",
    "5",
    "6",
    "7",
    "8 \u{b7} Smallest",
];
pub(crate) const FORMAT_LABELS: [&str; 7] = [
    "Same as source",
    "8-bit unsigned",
    "16-bit integer",
    "24-bit integer",
    "32-bit integer",
    "32-bit float",
    "64-bit float",
];

/// Pick item `ix` of `labels` in the select called `id`.
pub(crate) fn pick(
    cx: &mut TestAppContext,
    rig: &Rig,
    id: &'static str,
    labels: &[&str],
    ix: usize,
) {
    pick_label(cx, rig, id, labels[ix]);
}

/// Pick the item called `label` of the select `id` with the keyboard, as a user can: the
/// select shows its popup rows lazily, so arrow keys are the dependable way in.
pub(crate) fn pick_label(cx: &mut TestAppContext, rig: &Rig, id: &'static str, label: &str) {
    let (items, shown) = rig.workspace.read_with(cx, |w, cx| {
        let dialog = w.convert_dialog().expect("the sheet is open").read(cx);
        (dialog.select_labels(id, cx), dialog.shown_label(id, cx))
    });
    let ix = items
        .iter()
        .position(|l| l == label)
        .unwrap_or_else(|| panic!("{id} has no item {label:?}: {items:?}"));
    let at = shown.and_then(|text| items.iter().position(|l| *l == text));
    cx.update_window(rig.window, |_, window, cx| {
        window.within(id).click("input", cx);
        match at {
            Some(at) if ix >= at => (0..ix - at).for_each(|_| window.press("down", cx)),
            Some(at) => (0..at - ix).for_each(|_| window.press("up", cx)),
            None => (0..=ix).for_each(|_| window.press("down", cx)),
        }
        window.press("enter", cx);
    })
    .expect("open");
    step(cx, 3);
    draw(cx, rig.window);
}

/// Replace the output path the way a user does: focus the field, select all, type.
pub(crate) fn type_output(cx: &mut TestAppContext, rig: &Rig, path: &str) {
    click(cx, rig.window, "sheet-output");
    cx.update_window(rig.window, |_, window, cx| {
        window.press("secondary-a", cx);
        window.input(path, cx);
    })
    .expect("open");
    cx.run_until_parked();
    draw(cx, rig.window);
}
