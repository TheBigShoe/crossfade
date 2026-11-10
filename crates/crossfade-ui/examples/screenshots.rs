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

use crossfade_app::queue::RunnerEvent;
use crossfade_ui::prelude::gpui_test::TestWindowExt;
use crossfade_ui::prelude::*;
use crossfade_ui::scripted::Script;
use crossfade_ui::{Workspace, init};

const WINDOW: (f32, f32) = (1180., 760.);
const NARROW: (f32, f32) = (900., 600.);
const FRAME: Duration = Duration::from_millis(8);

/// One picture: its file name, its window size and what to do to the stage before it is
/// taken.
struct Scene {
    file: &'static str,
    size: (f32, f32),
    drive: fn(&mut Stage),
}

const SCENES: &[Scene] = &[
    Scene {
        file: "01-empty-dark.png",
        size: WINDOW,
        drive: |stage| stage.set_theme(ThemeMode::Dark),
    },
    Scene {
        file: "02-empty-light.png",
        size: WINDOW,
        drive: |stage| stage.set_theme(ThemeMode::Light),
    },
    Scene {
        file: "03-files-inspector-dark.png",
        size: WINDOW,
        drive: |stage| {
            stage.set_theme(ThemeMode::Dark);
            stage.add(&[
                ("interview-take-1.wav", 48_000, 2),
                ("room-tone.wav", 44_100, 1),
            ]);
            stage.add(&[("mix-master-final-v3-really-final.wav", 96_000, 2)]);
        },
    },
    Scene {
        file: "04-convert-sheet-dark.png",
        size: WINDOW,
        drive: |stage| {
            stage.set_theme(ThemeMode::Dark);
            stage.add(&[("interview-take-1.wav", 48_000, 2)]);
            stage.convert();
        },
    },
    Scene {
        file: "05-convert-sheet-problem-dark.png",
        size: WINDOW,
        drive: |stage| {
            stage.set_theme(ThemeMode::Dark);
            stage.add(&[("interview-take-1.wav", 48_000, 2)]);
            // The default output already exists, so the sheet warns.
            stage.write_bytes("interview-take-1-copy.wav", b"older conversion");
            stage.convert();
        },
    },
    Scene {
        file: "06-queue-running-dark.png",
        size: WINDOW,
        drive: |stage| {
            stage.set_theme(ThemeMode::Dark);
            stage.add(&[("interview-take-1.wav", 48_000, 2)]);
            stage.enqueue(&["interview-take-1.wav", "take-1-24bit.wav", "s24"]);
            stage.enqueue(&["interview-take-1.wav", "take-1-float.wav", "f32"]);
            stage.send(
                0,
                RunnerEvent::Progress {
                    frames_done: 21_000_000,
                    frames_total: Some(50_000_000),
                },
            );
        },
    },
    Scene {
        file: "07-queue-mixed-light.png",
        size: WINDOW,
        drive: |stage| {
            stage.set_theme(ThemeMode::Light);
            stage.add(&[("interview-take-1.wav", 48_000, 2)]);
            for out in ["a.wav", "b.wav", "c.wav", "d.wav", "e.wav"] {
                stage.enqueue(&["interview-take-1.wav", out, "s16"]);
            }
            stage.send(0, RunnerEvent::Finished { frames: 6_000_000 });
            stage.send(
                1,
                RunnerEvent::Failed {
                    message: "disk full while writing b.wav".into(),
                },
            );
            stage.click("queue-cancel-2");
            stage.send(2, RunnerEvent::Cancelled);
            stage.send(
                3,
                RunnerEvent::Progress {
                    frames_done: 30,
                    frames_total: Some(100),
                },
            );
        },
    },
    Scene {
        file: "08-failed-input-dark.png",
        size: WINDOW,
        drive: |stage| {
            stage.set_theme(ThemeMode::Dark);
            stage.add(&[("interview-take-1.wav", 48_000, 2)]);
            stage.write_bytes("broken-recording.wav", b"RIFF not really a wave file");
            stage.add_raw(&["broken-recording.wav"]);
        },
    },
    Scene {
        file: "09-narrow-900x600-dark.png",
        size: NARROW,
        drive: |stage| {
            stage.set_theme(ThemeMode::Dark);
            stage.add(&[
                ("interview-take-1.wav", 48_000, 2),
                (
                    "a-rather-long-file-name-for-the-narrow-window.wav",
                    96_000,
                    2,
                ),
            ]);
            stage.enqueue(&[
                "interview-take-1.wav",
                "a-rather-long-file-name-for-the-narrow-window-24bit.wav",
                "s24",
            ]);
            stage.send(
                0,
                RunnerEvent::Progress {
                    frames_done: 1,
                    frames_total: Some(3),
                },
            );
        },
    },
];

/// A scratch directory for generated inputs, removed when the scene ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(scene: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("crossfade-shots-{scene}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the scratch directory");
        Self(dir)
    }

    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A 16-bit PCM WAV of `frames` frames of a quiet ramp.
fn wav_s16(rate: u32, channels: u16, frames: u32) -> Vec<u8> {
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
    v.resize(v.len() + data_len as usize, 0);
    v
}

/// One scene's app, window and workspace.
struct Stage {
    workspace: Entity<Workspace>,
    window: AnyWindowHandle,
    cx: HeadlessAppContext,
    script: Script,
    dir: Scratch,
}

impl Stage {
    fn open(scene: &Scene, text_system: Arc<dyn PlatformTextSystem>) -> Self {
        // The app's icon set, as in the binary, so SVG icons draw. Keep this in step with
        // `src/main.rs`.
        let mut cx = HeadlessAppContext::with_platform(
            text_system,
            Arc::new(Assets),
            platform::current_headless_renderer,
        );
        let (width, height) = scene.size;
        let (script, options) = Script::options();
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
            script,
            dir: Scratch::new(scene.file.trim_end_matches(".png")),
        }
    }

    /// Switch gpui-kit's built-in theme.
    fn set_theme(&mut self, mode: ThemeMode) {
        self.cx.update(|cx| Theme::change(mode, None, cx));
        self.cx.run_until_parked();
    }

    fn write_bytes(&mut self, name: &str, bytes: &[u8]) {
        std::fs::write(self.dir.path(name), bytes).expect("write a scratch file");
    }

    /// Generate `(name, rate, channels)` WAVs, one second long, and add them as the
    /// workspace's inputs.
    fn add(&mut self, files: &[(&str, u32, u16)]) {
        let mut names = Vec::new();
        for (name, rate, channels) in files {
            self.write_bytes(name, &wav_s16(*rate, *channels, rate * 3));
            names.push(*name);
        }
        self.add_raw(&names);
    }

    fn add_raw(&mut self, names: &[&str]) {
        let paths: Vec<PathBuf> = names
            .iter()
            .map(|n| PathBuf::from(self.dir.path(n)))
            .collect();
        let workspace = self.workspace.clone();
        self.cx
            .update_window(self.window, |_, window, cx| {
                workspace.update(cx, |workspace, cx| workspace.add_paths(paths, window, cx));
            })
            .expect("the window is open");
        self.cx.run_until_parked();
    }

    /// Open the convert sheet for the selected file.
    fn convert(&mut self) {
        let workspace = self.workspace.clone();
        self.cx
            .update_window(self.window, |_, window, cx| {
                workspace.update(cx, |workspace, cx| workspace.open_convert(window, cx));
            })
            .expect("the window is open");
        self.cx.run_until_parked();
        self.cx.advance_clock(FRAME);
        self.cx.run_until_parked();
    }

    /// Queue `[input, output, sample format]` as the CLI would spell it.
    fn enqueue(&mut self, [input, output, format]: &[&str; 3]) {
        let args = [
            "convert".to_owned(),
            self.dir.path(input),
            "-o".to_owned(),
            self.dir.path(output),
            "--sample-format".to_owned(),
            (*format).to_owned(),
        ];
        let request = crossfade_cli::request_from_args(args)
            .expect("valid arguments")
            .expect("a convert request");
        let workspace = self.workspace.clone();
        self.cx.update(|cx| {
            workspace.update(cx, |workspace, cx| workspace.enqueue(request, cx));
        });
        self.cx.run_until_parked();
    }

    /// Report on the `start`-th job the runner was given.
    fn send(&mut self, start: usize, event: RunnerEvent) {
        self.script.send(start, event);
        self.cx.run_until_parked();
        self.draw().expect("draw");
    }

    fn click(&mut self, id: &'static str) {
        self.draw().expect("draw");
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.click(id, cx);
                // Park the pointer on something inert so no hover state is in the picture.
                window.hover("status-text", cx);
            })
            .expect("the window is open");
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
    let mut stage = Stage::open(scene, text_system);
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
