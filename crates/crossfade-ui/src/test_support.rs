//! Headless-window helpers shared by the UI tests.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

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

/// A window over a scripted runner and a scratch directory.
pub(crate) struct Rig {
    pub(crate) window: AnyWindowHandle,
    pub(crate) workspace: Entity<Workspace>,
    pub(crate) script: Script,
    pub(crate) dir: TempDir,
}

pub(crate) fn open_rig(cx: &mut TestAppContext) -> Rig {
    let (script, options) = Script::options();
    let (window, workspace) = open_with(cx, options);
    Rig {
        window,
        workspace,
        script,
        dir: TempDir::new("rig"),
    }
}

/// Open a headless window holding a [`Workspace`] over `options`, with the app
/// initialised as `main` does.
pub(crate) fn open_with(
    cx: &mut TestAppContext,
    options: WorkspaceOptions,
) -> (AnyWindowHandle, Entity<Workspace>) {
    cx.update(|cx| {
        // Animations run on the wall clock, not the test clock; with reduced motion
        // they show their end state from the first frame.
        cx.set_reduce_motion(true);
        init(cx);
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
