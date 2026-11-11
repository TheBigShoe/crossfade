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
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use crossfade_app::FsIo;
use crossfade_app::analysis::{Analyzer, State};
use crossfade_app::queue::RunnerEvent;
use crossfade_core::{Limits, SampleFormat};
use crossfade_ui::config::{self, Config, ConfigPiece};
use crossfade_ui::prelude::gpui_test::TestWindowExt;
use crossfade_ui::prelude::*;
use crossfade_ui::scripted::Script;
use crossfade_ui::{Workspace, init};

// The tests' fixture writers, so a picture shows what a real conversion produces.
#[path = "../src/fixtures.rs"]
mod fixtures;

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
        file: "10-settings-problem-dark.png",
        size: WINDOW,
        drive: |stage| {
            stage.set_theme(ThemeMode::Dark);
            stage.add(&[("interview-take-1.wav", 48_000, 2)]);
            stage.break_settings();
        },
    },
    Scene {
        file: "11-flac-inspector-tags-dark.png",
        size: WINDOW,
        drive: |stage| {
            stage.set_theme(ThemeMode::Dark);
            let tone = fixtures::flac(44_100, 2, SampleFormat::S16, 44_100 * 3);
            let master = fixtures::programme_flac_20_bit(48_000, 8);
            let tagged = fixtures::tagged(
                &master,
                &[
                    ("TITLE", "Night Ferry"),
                    ("ARTIST", "Mara Voss"),
                    ("ARTIST", "The Harbour Set"),
                    ("ALBUM", "Low Tide"),
                    ("DATE", "2025"),
                    ("TRACKNUMBER", "4"),
                ],
            );
            stage.add_bytes(&[
                ("room-tone.flac", tone),
                ("night-ferry-master.flac", tagged),
            ]);
        },
    },
    Scene {
        file: "12-convert-sheet-flac-dark.png",
        size: WINDOW,
        drive: |stage| {
            stage.set_theme(ThemeMode::Dark);
            stage.add(&[("interview-take-1.wav", 48_000, 2)]);
            stage.convert();
            stage.choose("sheet-container", 1);
        },
    },
    Scene {
        file: "13-convert-sheet-flac-hint-dark.png",
        size: WINDOW,
        drive: |stage| {
            stage.set_theme(ThemeMode::Dark);
            let float = fixtures::wav(48_000, 2, SampleFormat::F32, 48_000 * 3);
            stage.add_bytes(&[("mix-bus-float.wav", float)]);
            stage.convert();
            stage.choose("sheet-container", 1);
        },
    },
    Scene {
        file: "14-convert-sheet-copy-light.png",
        size: WINDOW,
        drive: |stage| {
            stage.set_theme(ThemeMode::Light);
            let tone = fixtures::flac(44_100, 2, SampleFormat::S16, 44_100 * 3);
            stage.add_bytes(&[("room-tone.flac", tone)]);
            stage.convert();
            stage.choose("sheet-container", 1);
            stage.click("sheet-copy");
        },
    },
    Scene {
        file: "15-convert-sheet-unsupported-dark.png",
        size: WINDOW,
        drive: |stage| {
            stage.set_theme(ThemeMode::Dark);
            // The engine reads nine channels; FLAC holds eight.
            let wide = fixtures::wav(48_000, 9, SampleFormat::S16, 48_000 * 2);
            stage.add_bytes(&[("surround-9ch-capture.wav", wide)]);
            stage.convert();
            stage.choose("sheet-container", 1);
        },
    },
    Scene {
        file: "16-convert-sheet-flac-hint-narrow-dark.png",
        size: NARROW,
        drive: |stage| {
            stage.set_theme(ThemeMode::Dark);
            let float = fixtures::wav(48_000, 2, SampleFormat::F32, 48_000 * 3);
            stage.add_bytes(&[("mix-bus-float.wav", float)]);
            stage.convert();
            stage.choose("sheet-container", 1);
        },
    },
    Scene {
        file: "18-convert-sheet-ogg-flac-dark.png",
        size: WINDOW,
        drive: |stage| {
            stage.set_theme(ThemeMode::Dark);
            let take = fixtures::wav(48_000, 2, SampleFormat::S24, 48_000 * 3);
            stage.add_bytes(&[("interview-take-1.wav", take)]);
            stage.convert();
            stage.choose("sheet-container", 3);
        },
    },
    Scene {
        file: "17-quit-confirm-dark.png",
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
            stage.quit();
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
    Scene {
        file: "19-waveform-dark.png",
        size: WINDOW,
        drive: |stage| {
            stage.set_theme(ThemeMode::Dark);
            stage.add_programme("interview-take-1.wav");
            stage.wait_analysis();
        },
    },
    Scene {
        file: "20-waveform-light.png",
        size: WINDOW,
        drive: |stage| {
            stage.set_theme(ThemeMode::Light);
            stage.add_programme("interview-take-1.wav");
            stage.wait_analysis();
        },
    },
    Scene {
        file: "21-waveform-zoomed-dark.png",
        size: WINDOW,
        drive: |stage| {
            stage.set_theme(ThemeMode::Dark);
            stage.add_programme("interview-take-1.wav");
            stage.wait_analysis();
            stage.click("waveform-strip");
            for _ in 0..3 {
                stage.press("+");
            }
            stage.press("right");
            stage.press("right");
        },
    },
    Scene {
        file: "22-waveform-analysing-narrow-dark.png",
        size: NARROW,
        drive: |stage| {
            stage.set_theme(ThemeMode::Dark);
            stage.settings_text("{ \"analyze_on_select\": false }");
            stage.add_programme("interview-take-1.wav");
            stage.analyse_partway("interview-take-1.wav", 42);
        },
    },
    Scene {
        file: "23-convert-sheet-adjust-collapsed-dark.png",
        size: WINDOW,
        drive: |stage| {
            stage.set_theme(ThemeMode::Dark);
            stage.add_programme("interview-take-1.wav");
            stage.convert();
            stage.choose("sheet-rate", 1);
            stage.adjust_normalize_and_fade();
            stage.click("sheet-adjust");
        },
    },
    Scene {
        file: "24-convert-sheet-adjust-expanded-dark.png",
        size: WINDOW,
        drive: |stage| {
            stage.set_theme(ThemeMode::Dark);
            stage.add_programme("interview-take-1.wav");
            stage.convert();
            stage.choose("sheet-rate", 1);
            stage.adjust_normalize_and_fade();
            stage.type_into("sheet-trim-start", "0:01");
            stage.type_into("sheet-trim-end", "0:06");
        },
    },
    Scene {
        file: "25-convert-sheet-adjust-problem-dark.png",
        size: WINDOW,
        drive: |stage| {
            stage.set_theme(ThemeMode::Dark);
            stage.add_programme("interview-take-1.wav");
            stage.convert();
            stage.click("sheet-adjust");
            stage.type_into("sheet-trim-start", "0:06");
            stage.type_into("sheet-trim-end", "0:02");
        },
    },
    Scene {
        file: "26-convert-sheet-adjust-narrow-light.png",
        size: NARROW,
        drive: |stage| {
            stage.set_theme(ThemeMode::Light);
            stage.add_programme("interview-take-1.wav");
            stage.convert();
            stage.choose("sheet-rate", 1);
            stage.choose("sheet-channels", 1);
            stage.adjust_normalize_and_fade();
            stage.type_into("sheet-trim-start", "0:01");
            stage.type_into("sheet-trim-end", "0:06");
        },
    },
    Scene {
        file: "27-queue-clipped-dark.png",
        size: WINDOW,
        drive: |stage| {
            stage.set_theme(ThemeMode::Dark);
            stage.add(&[("interview-take-1.wav", 48_000, 2)]);
            for out in ["take-1-louder.wav", "take-1-24bit.wav", "take-1-float.wav"] {
                stage.enqueue(&["interview-take-1.wav", out, "s16"]);
            }
            stage.send(0, RunnerEvent::Clipped { samples: 12_340 });
            stage.send(0, RunnerEvent::Finished { frames: 6_000_000 });
            stage.send(1, RunnerEvent::Finished { frames: 6_000_000 });
            stage.send(
                2,
                RunnerEvent::Progress {
                    frames_done: 21_000_000,
                    frames_total: Some(50_000_000),
                },
            );
        },
    },
    Scene {
        file: "28-queue-normalised-dark.png",
        size: WINDOW,
        drive: |stage| {
            stage.set_theme(ThemeMode::Dark);
            stage.add_programme("interview-take-1.wav");
            for out in ["take-1-loud.wav", "take-1-level.wav"] {
                stage.enqueue(&["interview-take-1.wav", out, "s16"]);
            }
            stage.send(0, RunnerEvent::Measuring);
            stage.send(
                0,
                RunnerEvent::Normalized(crossfade_graph::Normalization {
                    measured_lufs: Some(-23.0),
                    target_lufs: -16.0,
                    gain_db: 5.7,
                    limited_by_ceiling: true,
                    ceiling_dbtp: -1.0,
                }),
            );
            stage.send(0, RunnerEvent::Finished { frames: 384_000 });
            stage.send(1, RunnerEvent::Measuring);
            stage.send(
                1,
                RunnerEvent::Progress {
                    frames_done: 40,
                    frames_total: Some(200),
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

    /// The scene's own config directory: the real one is never read.
    fn config(&self) -> PathBuf {
        self.0.join("config")
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
    /// Parked analysis workers; dropping one lets its worker go on.
    gates: Vec<mpsc::Sender<()>>,
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
        let dir = Scratch::new(scene.file.trim_end_matches(".png"));
        let paths = appconfig::ConfigPaths::new(dir.config());
        let (window, workspace) = cx
            .update(|cx| {
                init(cx);
                // The bundled Crossfade themes, from a scratch config directory; never
                // `config::start` and never the platform's directory.
                config::install(Config::load(&paths, true), cx);
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
            dir,
            gates: Vec::new(),
        }
    }

    /// Follow a dark or light system appearance: the bundled themes switch with it.
    fn set_theme(&mut self, mode: ThemeMode) {
        let dark = mode == ThemeMode::Dark;
        self.cx.update(|cx| config::set_appearance(dark, cx));
        self.cx.run_until_parked();
    }

    /// Save a `settings.json` that does not parse and reload it, so the status bar says so.
    fn break_settings(&mut self) {
        std::fs::create_dir_all(self.dir.config()).expect("create the config directory");
        std::fs::write(
            self.dir.config().join("settings.json"),
            "{ \"ui_font_size\": ",
        )
        .expect("write settings.json");
        self.cx
            .update(|cx| config::reload(ConfigPiece::Settings, cx));
        self.cx.run_until_parked();
    }

    /// Save `settings.json` as `text` and reload it.
    fn settings_text(&mut self, text: &str) {
        std::fs::create_dir_all(self.dir.config()).expect("create the config directory");
        std::fs::write(self.dir.config().join("settings.json"), text).expect("write settings.json");
        self.cx
            .update(|cx| config::reload(ConfigPiece::Settings, cx));
        self.cx.run_until_parked();
    }

    /// Add a few seconds of programme-like stereo audio under `name`.
    fn add_programme(&mut self, name: &str) {
        self.add_bytes(&[(name, fixtures::programme_wav(48_000, 8))]);
    }

    /// Wait for the shown file's analysis to end, then deliver its last wake-up.
    fn wait_analysis(&mut self) {
        let strip = self.workspace.read_with(&self.cx, |w, _| w.strip().clone());
        strip.read_with(&self.cx, |s, _| s.wait());
        self.cx.run_until_parked();
        self.draw().expect("draw");
    }

    /// Show `name` mid-analysis: the worker parks once `percent` of the file is analysed and
    /// stays there until the scene ends, so the picture is the same every run.
    fn analyse_partway(&mut self, name: &str, percent: u64) {
        let path = self.dir.path(name);
        let (hold, held) = mpsc::channel::<()>();
        let held = Mutex::new(held);
        let strip = self.workspace.read_with(&self.cx, |w, _| w.strip().clone());
        let (reached, at_gate) = mpsc::channel::<()>();
        let reached = Mutex::new(reached);
        self.cx.update(|cx| {
            strip.update(cx, |strip, cx| {
                strip.select_with(None, true, cx, |_| unreachable!("nothing is shown"));
                strip.select_with(Some(&path), true, cx, |p| {
                    Analyzer::start(p, Box::new(FsIo::new()), Limits::default(), move |snap| {
                        let total = snap.stream.as_ref().and_then(|s| s.frames).unwrap_or(0);
                        if snap.state == State::Running && snap.frames_done * 100 >= total * percent
                        {
                            let _ = reached.lock().expect("lock").send(());
                            let _ = held.lock().expect("lock").recv();
                        }
                    })
                });
            });
        });
        at_gate.recv().expect("the worker reaches the gate");
        self.gates.push(hold);
        self.cx.run_until_parked();
        self.draw().expect("draw");
    }

    /// Press `keys` in the window.
    fn press(&mut self, keys: &str) {
        self.draw().expect("draw");
        self.cx
            .update_window(self.window, |_, window, cx| window.press(keys, cx))
            .expect("the window is open");
        self.cx.run_until_parked();
    }

    fn write_bytes(&mut self, name: &str, bytes: &[u8]) {
        std::fs::write(self.dir.path(name), bytes).expect("write a scratch file");
    }

    /// Generate `(name, rate, channels)` WAVs, three seconds long, and add them as the
    /// workspace's inputs.
    fn add(&mut self, files: &[(&str, u32, u16)]) {
        let mut names = Vec::new();
        for (name, rate, channels) in files {
            // Programme-like audio where we have it (stereo), so the strip is not a flat line.
            let bytes = if *channels == 2 {
                fixtures::programme_wav(*rate, 3)
            } else {
                wav_s16(*rate, *channels, rate * 3)
            };
            self.write_bytes(name, &bytes);
            names.push(*name);
        }
        self.add_raw(&names);
    }

    fn add_bytes(&mut self, files: &[(&str, Vec<u8>)]) {
        let mut names = Vec::new();
        for (name, bytes) in files {
            self.write_bytes(name, bytes);
            names.push(name.as_ref());
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

    /// Ask the app to quit, as Cmd-Q does.
    fn quit(&mut self) {
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.dispatch_action(Box::new(crossfade_ui::actions::Quit), cx);
            })
            .expect("the window is open");
        self.cx.run_until_parked();
        for _ in 0..3 {
            self.cx.advance_clock(FRAME);
            self.cx.run_until_parked();
        }
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

    /// Pick an item of a select in the open sheet by keyboard: `downs` arrow presses from
    /// whatever it shows, then Enter.
    fn choose(&mut self, id: &'static str, downs: usize) {
        self.draw().expect("draw");
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.within(id).click("input", cx);
                for _ in 0..downs {
                    window.press("down", cx);
                }
                window.press("enter", cx);
            })
            .expect("the window is open");
        self.cx.run_until_parked();
        self.cx.advance_clock(FRAME);
        self.cx.run_until_parked();
    }

    /// Draw a few frames so layout settles, then read the window back and write it to
    /// `path` as a PNG.
    fn capture(&mut self, path: &Path) -> Result<(u32, u32), String> {
        // A scene that shows a finished analysis must not race the worker; one that parked
        // its worker on purpose is the picture.
        if self.gates.is_empty() {
            self.wait_analysis();
        }
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

impl Stage {
    /// Replace what a field of the open sheet holds: focus it, select all, type.
    fn type_into(&mut self, id: &'static str, text: &str) {
        self.draw().expect("draw");
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.click(id, cx);
                window.press("secondary-a", cx);
                window.input(text, cx);
            })
            .expect("the window is open");
        self.cx.run_until_parked();
    }

    /// Open Adjust and set Normalize -16 LUFS and a 3 s fade out.
    fn adjust_normalize_and_fade(&mut self) {
        self.click("sheet-adjust");
        self.click("sheet-level-normalize");
        self.type_into("sheet-level-value", "-16");
        self.type_into("sheet-fade-out", "3");
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
