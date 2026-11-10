//! Screenshots of the real workspace, rendered offscreen: `just screenshots`.
//!
//! Each scene opens the workspace the way the headless tests do (GPUI's test platform),
//! puts it in a state, then draws a frame and reads it back from a Metal texture. No
//! window reaches the screen and no screen-recording permission is needed. Text is shaped
//! by the platform's text system, as in the app. Windows are 2x, like a Retina display's:
//! a 1180x760 window is a 2360x1520 PNG.
//!
//! It is an example rather than a test: AppKit, which the text system is reached through,
//! must be set up on the main thread, and a picture is for a person to look at. `cargo
//! test` still builds it, so it keeps up with the UI's API. It only runs on macOS;
//! elsewhere it says so and exits.
//!
//! `cargo run -p crossfade-ui --example screenshots -- [--out DIR] [NAME...]` writes to
//! `target/screenshots/` by default; each NAME keeps the scenes whose file name contains it.
//! To add a scene, write a `fn(&mut Stage)` and add a line to [`SCENES`].

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossfade_ui::prelude::gpui_test::TestWindowExt;
use crossfade_ui::prelude::*;
use crossfade_ui::scripted::Script;
use crossfade_ui::{Workspace, init};

const WINDOW: (f32, f32) = (1180., 760.);
const FRAME: Duration = Duration::from_millis(8);

/// One picture: its file name and what to do to the stage before it is taken.
struct Scene {
    file: &'static str,
    drive: fn(&mut Stage),
}

const SCENES: &[Scene] = &[
    Scene {
        file: "01-empty-dark.png",
        drive: |stage| stage.set_theme(ThemeMode::Dark),
    },
    Scene {
        file: "02-empty-light.png",
        drive: |stage| stage.set_theme(ThemeMode::Light),
    },
];

/// One scene's app, window and workspace.
struct Stage {
    #[expect(
        dead_code,
        reason = "scenes will read the workspace once they drive it"
    )]
    workspace: Entity<Workspace>,
    window: AnyWindowHandle,
    cx: HeadlessAppContext,
}

impl Stage {
    fn open(text_system: Arc<dyn PlatformTextSystem>) -> Self {
        // The app's icon set, as in the binary, so SVG icons draw. Keep this in step with
        // `src/main.rs`.
        let mut cx = HeadlessAppContext::with_platform(
            text_system,
            Arc::new(Assets),
            platform::current_headless_renderer,
        );
        let (width, height) = WINDOW;
        let (_, options) = Script::options();
        let (window, workspace) = cx
            .update(|cx| {
                init(cx);
                // Animations show their end state, so the picture does not depend on how
                // much real time has passed.
                cx.set_reduce_motion(true);
                kit_open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds {
                            origin: Point::default(),
                            size: size(px(width), px(height)),
                        })),
                        ..Default::default()
                    },
                    cx,
                    |window, cx| cx.new(|cx| Workspace::new(options, window, cx)),
                )
            })
            .expect("open the workspace window");
        cx.run_until_parked();
        Self {
            workspace,
            window,
            cx,
        }
    }

    /// Switch gpui-kit's built-in theme.
    fn set_theme(&mut self, mode: ThemeMode) {
        self.cx.update(|cx| Theme::change(mode, None, cx));
        self.cx.run_until_parked();
    }

    /// Draw a few frames so layout settles, then read the window back and write it to
    /// `path` as a PNG.
    fn capture(&mut self, path: &Path) -> Result<(u32, u32), String> {
        for _ in 0..3 {
            self.draw()?;
            self.cx.run_until_parked();
            self.cx.advance_clock(FRAME);
        }
        self.cx.run_until_parked();
        self.draw()?;
        let image = self
            .cx
            .capture_screenshot(self.window)
            .map_err(|error| format!("capture_screenshot failed: {error:#}"))?;
        image
            .save(path)
            .map_err(|error| format!("could not write {}: {error}", path.display()))?;
        Ok((image.width(), image.height()))
    }

    fn draw(&mut self) -> Result<(), String> {
        self.cx
            .update_window(self.window, |_, window, cx| window.render_frame(cx))
            .map_err(|error| format!("the window is gone: {error:#}"))
    }
}

struct Args {
    out: PathBuf,
    names: Vec<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut out = PathBuf::from("target/screenshots");
    let mut names = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out = args.next().ok_or("--out needs a directory")?.into(),
            flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}")),
            name => names.push(name.to_owned()),
        }
    }
    Ok(Args { out, names })
}

fn take(
    scene: &Scene,
    text_system: Arc<dyn PlatformTextSystem>,
    path: &Path,
) -> Result<(u32, u32), String> {
    let mut stage = Stage::open(text_system);
    (scene.drive)(&mut stage);
    stage.capture(path)
}

fn run(args: Args) -> bool {
    let scenes: Vec<&Scene> = SCENES
        .iter()
        .filter(|scene| {
            args.names.is_empty() || args.names.iter().any(|n| scene.file.contains(n.as_str()))
        })
        .collect();
    if scenes.is_empty() {
        eprintln!("screenshots: no scene matches {:?}", args.names);
        return false;
    }
    std::fs::create_dir_all(&args.out).expect("create the output directory");
    if args.names.is_empty() {
        // A full run replaces every picture, so none is left over from an older one.
        for entry in std::fs::read_dir(&args.out).into_iter().flatten().flatten() {
            if entry.path().extension().is_some_and(|ext| ext == "png") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    let out = args.out.canonicalize().unwrap_or(args.out);

    // One platform for the whole run; each scene gets its own app over its text system.
    let platform = platform::current_platform(true);
    let text_system = platform.text_system();
    let started = Instant::now();
    let mut failed = Vec::new();
    for scene in scenes {
        let path = out.join(scene.file);
        let began = Instant::now();
        let text_system = text_system.clone();
        match catch_unwind(AssertUnwindSafe(|| take(scene, text_system, &path))) {
            Ok(Ok((width, height))) => eprintln!(
                "  {:<24} {width}x{height}  {:.1}s",
                scene.file,
                began.elapsed().as_secs_f64()
            ),
            Ok(Err(error)) => {
                eprintln!("  {:<24} FAILED: {error}", scene.file);
                failed.push(scene.file);
            }
            Err(_) => {
                eprintln!("  {:<24} FAILED (panicked, see above)", scene.file);
                failed.push(scene.file);
            }
        }
    }
    eprintln!(
        "screenshots: wrote to {} in {:.1}s",
        out.display(),
        started.elapsed().as_secs_f64()
    );
    if !failed.is_empty() {
        eprintln!(
            "screenshots: {} failed: {}",
            failed.len(),
            failed.join(", ")
        );
    }
    failed.is_empty()
}

fn main() {
    if !cfg!(target_os = "macos") {
        eprintln!("screenshots: skipped; offscreen capture uses the macOS text system and Metal");
        return;
    }
    let args = match parse_args() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("screenshots: {error}");
            std::process::exit(2);
        }
    };
    if !run(args) {
        std::process::exit(1);
    }
}
