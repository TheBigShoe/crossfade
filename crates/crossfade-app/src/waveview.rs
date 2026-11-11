//! What the inspector's waveform strip shows and how its gestures move it: the visible range,
//! the ruler, the position bar, the lanes and the readout line. All arithmetic lives here; the
//! view forwards events and paints what these return.
//!
//! The range is `start` plus `frames_per_pixel` for each of `width` pixels. Frames per pixel is
//! a whole number from 1 (the narrowest) up to the value that fits the file, so a pixel column
//! is always a whole number of frames wide and the pyramid is asked the same question at every
//! column.

use crate::analysis::{Snapshot, State};
use crate::waveform::Pyramid;
use crossfade_loudness::to_db;

/// Fewest pixels between two ruler ticks.
const MIN_TICK_PX: f64 = 72.0;
/// A tick closer than this to the right edge has no room for its label.
const LABEL_ROOM_PX: f32 = 28.0;
/// Zoom factor of the `+` and `-` keys.
const KEY_ZOOM: f64 = 2.0;
/// Pan of an arrow key, as a fraction of the visible range.
const KEY_PAN: f64 = 0.1;
/// Scroll pixels that change the zoom by a factor of e.
const SCROLL_ZOOM_PX: f32 = 100.0;
/// True peak above this many dBTP is flagged.
const PEAK_WARN_DB: f64 = -1.0;

const STEPS: &[f64] = &[
    0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0,
    120.0, 300.0, 600.0, 900.0, 1800.0, 3600.0, 7200.0, 14_400.0, 43_200.0,
];

#[derive(Debug, Clone, PartialEq)]
pub struct Tick {
    /// Pixels from the left edge of the strip.
    pub x: f32,
    pub label: String,
}

/// How many lanes a file gets: one per channel for mono and stereo, otherwise one combined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanePlan {
    Separate(usize),
    Combined,
}

impl LanePlan {
    pub fn for_channels(channels: usize) -> Self {
        if channels <= 2 {
            Self::Separate(channels)
        } else {
            Self::Combined
        }
    }

    pub fn lanes(self) -> usize {
        match self {
            Self::Separate(n) => n,
            Self::Combined => 1,
        }
    }
}

/// One lane: a `(min, max)` per pixel column, `None` where nothing is analysed yet.
pub type Lane = Vec<Option<(f32, f32)>>;

#[derive(Debug, Clone, Default)]
pub struct WaveView {
    frames: u64,
    rate: u32,
    width: u32,
    start: u64,
    fpp: u64,
}

impl WaveView {
    pub fn new() -> Self {
        Self {
            fpp: 1,
            ..Self::default()
        }
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn start(&self) -> u64 {
        self.start
    }

    pub fn frames_per_pixel(&self) -> u64 {
        self.fpp
    }

    fn fit_fpp(&self) -> u64 {
        self.frames.div_ceil(u64::from(self.width.max(1))).max(1)
    }

    pub fn is_fit(&self) -> bool {
        self.fpp >= self.fit_fpp()
    }

    /// The whole file in view.
    pub fn fit(&mut self) {
        self.fpp = self.fit_fpp();
        self.start = 0;
    }

    /// A new file, or a known length that grew: a fitted view stays fitted, a zoomed one keeps
    /// its place.
    pub fn set_source(&mut self, frames: u64, rate: u32) {
        let was_fit = self.is_fit() || self.frames == 0;
        self.frames = frames;
        self.rate = rate;
        if was_fit {
            self.fit();
        } else {
            self.clamp();
        }
    }

    pub fn set_width(&mut self, px: u32) {
        if px == self.width {
            return;
        }
        let was_fit = self.is_fit();
        self.width = px;
        if was_fit {
            self.fit();
        } else {
            self.clamp();
        }
    }

    fn span(&self) -> u64 {
        self.fpp.saturating_mul(u64::from(self.width))
    }

    fn clamp(&mut self) {
        self.fpp = self.fpp.clamp(1, self.fit_fpp());
        self.start = self.start.min(self.frames.saturating_sub(self.span()));
    }

    /// The frames in view, `start..end`, the end held to the file's.
    pub fn visible(&self) -> (u64, u64) {
        (
            self.start,
            self.start.saturating_add(self.span()).min(self.frames),
        )
    }

    pub fn frame_at(&self, px: f32) -> u64 {
        let f = self.start as f64 + f64::from(px) * self.fpp as f64;
        f.clamp(0.0, self.frames as f64) as u64
    }

    pub fn x_of(&self, frame: u64) -> f32 {
        ((frame as f64 - self.start as f64) / self.fpp as f64) as f32
    }

    /// Zoom in by `factor` (above 1; below 1 zooms out) keeping the frame under `anchor_px`
    /// where it is. Always moves by at least one frame per pixel unless a limit stops it.
    pub fn zoom(&mut self, factor: f64, anchor_px: f32) {
        if self.frames == 0 || !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let old = self.fpp;
        let mut new = ((old as f64 / factor).round() as u64).max(1);
        if factor > 1.0 && new >= old {
            new = old.saturating_sub(1);
        } else if factor < 1.0 && new <= old {
            new = old + 1;
        }
        let new = new.clamp(1, self.fit_fpp());
        let anchor = self.start as f64 + f64::from(anchor_px) * old as f64;
        self.fpp = new;
        self.start = (anchor - f64::from(anchor_px) * new as f64).max(0.0) as u64;
        self.clamp();
    }

    fn centre(&self) -> f32 {
        self.width as f32 / 2.0
    }

    pub fn zoom_in(&mut self) {
        self.zoom(KEY_ZOOM, self.centre());
    }

    pub fn zoom_out(&mut self) {
        self.zoom(1.0 / KEY_ZOOM, self.centre());
    }

    /// A pinch: `delta` of 0.1 is ten per cent closer.
    pub fn pinch(&mut self, delta: f32, anchor_px: f32) {
        self.zoom(1.0 + f64::from(delta), anchor_px);
    }

    /// Scrolling with the zoom modifier: up (positive) zooms in.
    pub fn zoom_scroll(&mut self, dy: f32, anchor_px: f32) {
        self.zoom(f64::from(dy / SCROLL_ZOOM_PX).exp(), anchor_px);
    }

    /// Move the range by `dx` pixels, later in the file when positive.
    pub fn pan_px(&mut self, dx: f32) {
        let moved = self.start as f64 + f64::from(dx) * self.fpp as f64;
        self.start = moved.max(0.0) as u64;
        self.clamp();
    }

    /// An arrow key: a tenth of the visible range, `direction` 1 for later and -1 for earlier.
    pub fn pan_step(&mut self, direction: i32) {
        self.pan_px(direction as f32 * self.width as f32 * KEY_PAN as f32);
    }

    /// Where the visible range sits in the file, as fractions from 0 to 1; `None` when the
    /// whole file is in view.
    pub fn position(&self) -> Option<(f32, f32)> {
        if self.is_fit() || self.frames == 0 {
            return None;
        }
        let (a, b) = self.visible();
        Some((
            (a as f64 / self.frames as f64) as f32,
            (b as f64 / self.frames as f64) as f32,
        ))
    }

    /// The ruler: nice steps, at least [`MIN_TICK_PX`] apart, labelled `m:ss` (`h:mm:ss` for a
    /// file of an hour or more, decimals once a tick is under a second apart).
    pub fn ticks(&self) -> Vec<Tick> {
        if self.frames == 0 || self.rate == 0 || self.width == 0 {
            return Vec::new();
        }
        let rate = f64::from(self.rate);
        let px_per_second = rate / self.fpp as f64;
        let Some(&step) = STEPS.iter().find(|s| **s * px_per_second >= MIN_TICK_PX) else {
            return Vec::new();
        };
        let hours = self.frames as f64 / rate >= 3600.0;
        let first = (self.start as f64 / rate / step).ceil() as u64;
        let limit = self.width as f32 - LABEL_ROOM_PX;
        (first..)
            .map(|k| (k, (k as f64 * step * rate).round() as u64))
            .map(|(k, frame)| (k, self.x_of(frame)))
            .take_while(|(_, x)| *x <= limit)
            .map(|(k, x)| Tick {
                x,
                label: clock_label(k as f64 * step, step, hours),
            })
            .collect()
    }

    /// Pixel columns the file spans in view (fewer than the width when it ends sooner).
    pub fn columns(&self) -> usize {
        let reach = self.span().min(self.frames.saturating_sub(self.start));
        reach.div_ceil(self.fpp) as usize
    }

    /// What to paint: a [`Lane`] per [`LanePlan`] lane, one column per pixel as far as the file
    /// goes. Reads the pyramid only, never samples.
    pub fn draw_columns(&self, pyramid: &Pyramid) -> Vec<Lane> {
        let n = self.columns();
        match LanePlan::for_channels(pyramid.channels()) {
            LanePlan::Combined => vec![pyramid.columns_union(self.start, self.fpp, n)],
            LanePlan::Separate(lanes) => (0..lanes)
                .map(|c| pyramid.columns(c, self.start, self.fpp, n))
                .collect(),
        }
    }
}

fn clock_label(seconds: f64, step: f64, hours: bool) -> String {
    let decimals = if step >= 1.0 {
        0
    } else if step >= 0.1 {
        1
    } else if step >= 0.01 {
        2
    } else {
        3
    };
    let scale = 10u64.pow(decimals);
    let units = (seconds * scale as f64).round() as u64;
    let (whole, frac) = (units / scale, units % scale);
    let (h, m, s) = (whole / 3600, whole / 60 % 60, whole % 60);
    let mut out = if hours {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{}:{s:02}", whole / 60)
    };
    if decimals > 0 {
        out.push_str(&format!(".{frac:0width$}", width = decimals as usize));
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub text: String,
    /// The theme's warning colour, for this figure only.
    pub warn: bool,
}

/// The one line under the ruler, in pieces so a view can colour a figure.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Readout {
    pub segments: Vec<Segment>,
}

impl Readout {
    fn plain(text: impl Into<String>) -> Self {
        Self {
            segments: vec![Segment {
                text: text.into(),
                warn: false,
            }],
        }
    }

    pub fn text(&self) -> String {
        self.segments.iter().map(|s| s.text.as_str()).collect()
    }

    pub fn warns(&self) -> bool {
        self.segments.iter().any(|s| s.warn)
    }
}

/// `-16.2` with a true minus sign, and never `-0.0`.
fn db_text(value: f64) -> String {
    let text = format!("{value:.1}");
    match text.strip_prefix('-') {
        Some("0.0") => "0.0".to_owned(),
        Some(rest) => format!("\u{2212}{rest}"),
        None => text,
    }
}

/// The engine's error text without its leading category (`unsupported: `, `i/o error: `).
fn specific(reason: &str) -> &str {
    ["unsupported: ", "malformed input: ", "i/o error: "]
        .iter()
        .find_map(|prefix| reason.strip_prefix(prefix))
        .unwrap_or(reason)
}

pub fn readout(snapshot: &Snapshot) -> Readout {
    match &snapshot.state {
        State::Failed(reason) => Readout::plain(format!("Could not analyse: {}", specific(reason))),
        State::Cancelled => Readout::default(),
        State::Running => {
            let total = snapshot.stream.as_ref().and_then(|s| s.frames);
            match total.filter(|t| *t > 0) {
                Some(total) => {
                    let pct = (u128::from(snapshot.frames_done) * 100 / u128::from(total)).min(100);
                    Readout::plain(format!("Analysing\u{2026} {pct}%"))
                }
                None => Readout::plain("Analysing\u{2026}"),
            }
        }
        State::Done => {
            let l = &snapshot.loudness;
            let mut parts = Vec::new();
            if let Some(i) = l.integrated {
                parts.push(Segment {
                    text: format!("{} LUFS", db_text(i)),
                    warn: false,
                });
            }
            if let Some(r) = l.range {
                parts.push(Segment {
                    text: format!("{:.1} LU range", r),
                    warn: false,
                });
            }
            if let Some(tp) = l.true_peak.map(to_db).filter(|d| d.is_finite()) {
                parts.push(Segment {
                    text: format!("{} dBTP", db_text(tp)),
                    warn: tp > PEAK_WARN_DB,
                });
            }
            if parts.is_empty() {
                return Readout::plain(if l.sample_peak == 0.0 {
                    "Silent"
                } else {
                    "Too short to measure"
                });
            }
            let mut segments = Vec::new();
            for (n, part) in parts.into_iter().enumerate() {
                if n > 0 {
                    segments.push(Segment {
                        text: " \u{b7} ".into(),
                        warn: false,
                    });
                }
                segments.push(part);
            }
            Readout { segments }
        }
    }
}
