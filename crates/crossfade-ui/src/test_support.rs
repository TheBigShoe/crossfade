//! Headless-window helpers shared by the UI tests.

use std::time::Duration;

use crate::prelude::*;
use crate::workspace::{Workspace, init};

/// A laptop-sized test window.
pub(crate) const TEST_WINDOW: (f32, f32) = (1180., 760.);

/// About a frame at 120 Hz.
pub(crate) const FRAME: Duration = Duration::from_millis(8);

/// Open a headless window holding a [`Workspace`], with the app initialised as `main` does.
pub(crate) fn open_test_window(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
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
            |window, cx| cx.new(|cx| Workspace::new(window, cx)),
        )
        .expect("open test window")
    })
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
pub(crate) fn click(cx: &mut TestAppContext, window: AnyWindowHandle, id: &'static str) {
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
