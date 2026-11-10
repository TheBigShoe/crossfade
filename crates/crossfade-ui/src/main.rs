//! The Crossfade desktop app: opens the main window and quits when the last one closes.

use crossfade_ui::prelude::*;
use crossfade_ui::{init, open_main_window};

fn main() {
    // The icon set is registered here and in `examples/screenshots.rs`; change both
    // together. Without an asset source every SVG icon draws as nothing.
    application().with_assets(Assets).run(|cx| {
        init(cx);
        if open_main_window(cx).is_err() {
            cx.quit();
            return;
        }
        // macOS keeps an app alive with no windows; this one has nothing left to do.
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
    });
}
