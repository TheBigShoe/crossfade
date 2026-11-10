//! The Crossfade desktop app: opens the main window and quits when the last one closes.

use crossfade_ui::config;
use crossfade_ui::prelude::*;
use crossfade_ui::{init, open_main_window};

fn main() {
    let config_dir = match config::parse_args(std::env::args().skip(1)) {
        Ok(dir) => dir,
        Err(usage) => {
            eprintln!("{usage}");
            std::process::exit(2);
        }
    };
    // The icon set is registered here and in `examples/screenshots.rs`; change both
    // together. Without an asset source every SVG icon draws as nothing.
    application().with_assets(Assets).run(move |cx| {
        init(cx);
        // Loads the user's configuration over the bundled one `init` installed, and
        // watches it for changes.
        config::start(config::paths_for(config_dir), cx);
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
