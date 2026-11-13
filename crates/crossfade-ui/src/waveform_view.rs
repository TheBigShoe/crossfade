//! The inspector's waveform strip. It renders `crossfade_app::strip::Strip` and forwards
//! gestures to its `WaveView`; the arithmetic lives there.
//!
//! Painting asks the pyramid for one column per horizontal pixel per lane and draws a quad for
//! each; it never reads samples, and takes only what `Analyzer::with` gives (the worker holds
//! that lock for one decoded frame's blocks at a time).

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use crossfade_app::analysis::{Analyzer, Snapshot, State};
use crossfade_app::strip::{Strip, Wake};
use crossfade_app::waveview::{Lane, LanePlan, readout};
use crossfade_core::Limits;

use crate::actions::{
    WAVEFORM_CONTEXT, WaveformFit, WaveformPanLeft, WaveformPanRight, WaveformZoomIn,
    WaveformZoomOut,
};
use crate::chrome::LABEL_SIZE;
use crate::prelude::*;
use crate::workspace::{AudioOutput, IoFactory};

/// The height of the waveform lanes' background.
const STRIP_HEIGHT: Pixels = px(132.);
/// Space above and below the lanes, and between them.
const LANE_PAD: f32 = 6.;
const LANE_GAP: f32 = 6.;
/// A line of the small muted text under the strip.
const LINE_HEIGHT: Pixels = px(16.);
/// Pixels a wheel line is worth.
const LINE_PIXELS: f32 = 16.;

pub struct WaveformStrip {
    strip: Strip,
    focus: FocusHandle,
    io: IoFactory,
    wake: Arc<Wake>,
    sender: async_channel::Sender<()>,
    /// Where the lanes are, in window pixels: left edge and width, as last painted.
    left: Rc<Cell<f32>>,
    _wakes: Task<()>,
}

impl WaveformStrip {
    pub fn new(io: IoFactory, _audio: AudioOutput, cx: &mut Context<Self>) -> Self {
        let (sender, receiver) = async_channel::unbounded::<()>();
        let wake = Arc::new(Wake::default());
        let acknowledge = Arc::clone(&wake);
        let wakes = cx.spawn(async move |this, cx| {
            while receiver.recv().await.is_ok() {
                // Before the read in render, so an update after it asks again.
                acknowledge.acknowledge();
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    return;
                }
            }
        });
        Self {
            strip: Strip::default(),
            focus: cx.focus_handle(),
            io,
            wake,
            sender,
            left: Rc::new(Cell::new(0.)),
            _wakes: wakes,
        }
    }

    pub fn path(&self) -> Option<&str> {
        self.strip.path()
    }

    pub fn view(&self) -> &crossfade_app::waveview::WaveView {
        self.strip.view()
    }

    pub fn has_analysis(&self) -> bool {
        self.strip.analyzer().is_some()
    }

    /// Block until the shown file's analysis has ended: tests and screenshots, which must not
    /// race the worker.
    pub fn wait(&self) {
        if let Some(analyzer) = self.strip.analyzer() {
            analyzer.wait();
        }
    }

    /// The player's state, when there is a player.
    pub fn player_state(&self) -> Option<crossfade_app::player::State> {
        None
    }

    /// The playback position in source frames.
    pub fn position(&self) -> Option<u64> {
        None
    }

    /// Waits for the player's worker to catch up (tests and screenshots).
    pub fn settle(&self) {}

    pub fn is_following(&self) -> bool {
        false
    }

    pub fn is_scrubbing(&self) -> bool {
        false
    }

    /// The transport's time readout.
    pub fn transport_text(&self) -> Option<String> {
        None
    }

    pub fn toggle_playback(&mut self, _: &mut Context<Self>) {}

    pub fn to_start(&mut self, _: &mut Context<Self>) {}

    pub fn to_end(&mut self, _: &mut Context<Self>) {}

    /// The line under the ruler as plain text.
    pub fn readout_text(&self) -> Option<String> {
        let snapshot = self.strip.analyzer()?.snapshot();
        Some(readout(&snapshot).text())
    }

    fn starter(&self) -> impl Fn(&str) -> Analyzer + use<> {
        let (io, wake, sender) = (
            Arc::clone(&self.io),
            Arc::clone(&self.wake),
            self.sender.clone(),
        );
        move |path: &str| {
            let (wake, sender) = (Arc::clone(&wake), sender.clone());
            Analyzer::start(path, io(), Limits::default(), move |_| {
                // Coalesced: one pending wake-up however many reports arrive.
                if wake.request() {
                    let _ = sender.try_send(());
                }
            })
        }
    }

    /// Show `path`: a cached analysis at once, otherwise a new one when `auto`.
    pub fn select(&mut self, path: Option<&str>, auto: bool, cx: &mut Context<Self>) {
        let start = self.starter();
        self.select_with(path, auto, cx, start);
    }

    /// [`select`](Self::select) with the analysis `start` makes: a seam for tests and
    /// screenshots, which hand in an analysis they control.
    pub fn select_with(
        &mut self,
        path: Option<&str>,
        auto: bool,
        cx: &mut Context<Self>,
        start: impl FnOnce(&str) -> Analyzer,
    ) {
        self.strip.select(path, auto, start);
        cx.notify();
    }

    fn analyse(&mut self, cx: &mut Context<Self>) {
        let start = self.starter();
        self.strip.analyse(start);
        cx.notify();
    }

    fn edit(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut crossfade_app::waveview::WaveView),
    ) {
        f(self.strip.view_mut());
        cx.notify();
    }

    fn muted_line(&self, id: &'static str, text: String, cx: &App) -> AnyElement {
        div()
            .id(id)
            .test_support()
            .h(LINE_HEIGHT)
            .line_height(LINE_HEIGHT)
            .truncate()
            .text_size(LABEL_SIZE)
            .text_color(cx.theme().muted_foreground)
            .child(SharedString::from(text))
            .into_any_element()
    }

    fn lane_background(&self, cx: &App) -> Hsla {
        cx.theme().foreground.opacity(0.05)
    }

    fn idle(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, foreground) = (theme.muted_foreground, theme.foreground);
        div()
            .id("waveform-idle")
            .flex()
            .items_center()
            .justify_center()
            .w_full()
            .h(STRIP_HEIGHT)
            .rounded_md()
            .bg(self.lane_background(cx))
            .child(
                div()
                    .id("waveform-analyse")
                    .test_support()
                    .cursor_pointer()
                    .text_size(LABEL_SIZE + px(1.))
                    .text_color(muted)
                    .hover(move |style| style.text_color(foreground))
                    .child("Analyse")
                    .on_click(cx.listener(|this, _, _, cx| this.analyse(cx))),
            )
            .into_any_element()
    }

    fn drawn(&mut self, snapshot: &Snapshot, focused: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (wave, line, muted, warning) = (
            theme.primary.opacity(0.6),
            theme.muted_foreground.opacity(0.35),
            theme.muted_foreground,
            theme.warning,
        );
        let (frames, rate, channels) = match &snapshot.stream {
            Some(stream) => (
                stream.frames.unwrap_or(snapshot.frames_done),
                stream.audio.sample_rate(),
                usize::from(stream.audio.layout().channels()),
            ),
            None => (0, 0, 2),
        };
        self.strip.view_mut().set_source(frames, rate);
        let view = self.strip.view().clone();
        let lanes: Vec<Lane> = self
            .strip
            .analyzer()
            .map(|a| a.with(|analysis| view.draw_columns(&analysis.pyramid)))
            .unwrap_or_default();
        let planned = LanePlan::for_channels(channels).lanes().max(1);
        let known = view.frames() > 0;
        let columns = view.columns();
        let left = Rc::clone(&self.left);
        let entity = cx.entity();
        let background = self.lane_background(cx);
        let canvas = canvas(
            |bounds, _, _| bounds,
            move |_, bounds, window, cx| {
                left.set(f32::from(bounds.origin.x));
                let width = f32::from(bounds.size.width);
                entity.update(cx, |this, cx| {
                    let w = width.floor() as u32;
                    if this.strip.view().width() != w {
                        this.strip.view_mut().set_width(w);
                        cx.notify();
                    }
                });
                let lane_count = if lanes.is_empty() {
                    planned
                } else {
                    lanes.len()
                };
                let height = f32::from(bounds.size.height);
                let lane_h = ((height - 2. * LANE_PAD - (lane_count - 1) as f32 * LANE_GAP)
                    / lane_count as f32)
                    .max(2.);
                let (x0, y0) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
                let line_cols = if known { columns } else { width as usize };
                for lane in 0..lane_count {
                    let top = y0 + LANE_PAD + lane as f32 * (lane_h + LANE_GAP);
                    let mid = top + lane_h / 2.;
                    window.paint_quad(fill(
                        Bounds::new(
                            point(px(x0), px(mid - 0.5)),
                            size(px(line_cols as f32), px(1.)),
                        ),
                        line,
                    ));
                    let Some(columns) = lanes.get(lane) else {
                        continue;
                    };
                    for (i, column) in columns.iter().enumerate() {
                        let Some((lo, hi)) = column else {
                            continue;
                        };
                        let (hi, lo) = (hi.clamp(-1., 1.), lo.clamp(-1., 1.));
                        let y_top = mid - hi * lane_h / 2.;
                        let y_bottom = (mid - lo * lane_h / 2.).max(y_top + 1.);
                        window.paint_quad(fill(
                            Bounds::new(
                                point(px(x0 + i as f32), px(y_top)),
                                size(px(1.), px(y_bottom - y_top)),
                            ),
                            wave,
                        ));
                    }
                }
            },
        )
        .absolute()
        .size_full();
        let strip = div()
            .id("waveform-strip")
            .test_support()
            .key_context(WAVEFORM_CONTEXT)
            .track_focus(&self.focus)
            .relative()
            .w_full()
            .h(STRIP_HEIGHT)
            .rounded_md()
            .overflow_hidden()
            .bg(if focused {
                background.opacity(1.4)
            } else {
                background
            })
            .child(canvas)
            .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                window.focus(&this.focus, cx);
                if event.click_count() == 2 {
                    this.edit(cx, |view| view.fit());
                }
            }))
            .on_scroll(cx.listener(|this, event: &ScrollEvent, _, cx| {
                let delta = event.delta.pixel_delta(px(LINE_PIXELS));
                let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));
                let anchor = f32::from(event.position.x) - this.left.get();
                if event.modifiers.secondary(cx) {
                    this.edit(cx, |view| view.zoom_scroll(dy, anchor));
                    cx.stop_propagation();
                } else if dx.abs() > dy.abs() {
                    // Content follows the fingers: moving right shows earlier audio.
                    this.edit(cx, |view| view.pan_px(-dx));
                    cx.stop_propagation();
                }
            }))
            .on_pinch(cx.listener(|this, event: &PinchEvent, _, cx| {
                let anchor = f32::from(event.position.x) - this.left.get();
                this.edit(cx, |view| view.pinch(event.delta, anchor));
                cx.stop_propagation();
            }))
            .on_action(
                cx.listener(|this, _: &WaveformZoomIn, _, cx| this.edit(cx, |v| v.zoom_in())),
            )
            .on_action(
                cx.listener(|this, _: &WaveformZoomOut, _, cx| this.edit(cx, |v| v.zoom_out())),
            )
            .on_action(cx.listener(|this, _: &WaveformFit, _, cx| this.edit(cx, |v| v.fit())))
            .on_action(
                cx.listener(|this, _: &WaveformPanLeft, _, cx| this.edit(cx, |v| v.pan_step(-1))),
            )
            .on_action(
                cx.listener(|this, _: &WaveformPanRight, _, cx| this.edit(cx, |v| v.pan_step(1))),
            );

        let ruler = div()
            .id("waveform-ruler")
            .test_support()
            .relative()
            .w_full()
            .h(LINE_HEIGHT)
            .overflow_hidden()
            .text_size(LABEL_SIZE)
            .text_color(muted)
            .children(view.ticks().into_iter().map(|tick| {
                div()
                    .absolute()
                    .top_0()
                    .left(px(tick.x))
                    .whitespace_nowrap()
                    .line_height(LINE_HEIGHT)
                    .child(SharedString::from(tick.label))
            }));

        let position = div()
            .relative()
            .w_full()
            .h(px(2.))
            .children(view.position().map(|(a, b)| {
                let track = view.width() as f32;
                div()
                    .id("waveform-position")
                    .test_support()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w_full()
                    .h(px(2.))
                    .bg(muted.opacity(0.18))
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .h(px(2.))
                            .left(px(a * track))
                            .w(px(((b - a) * track).max(2.)))
                            .rounded_full()
                            .bg(wave),
                    )
            }));

        let line = readout(snapshot);
        let readout_line = h_flex()
            .id("waveform-readout")
            .test_support()
            .h(LINE_HEIGHT)
            .line_height(LINE_HEIGHT)
            .text_size(LABEL_SIZE)
            .children(line.segments.into_iter().map(|segment| {
                div()
                    .whitespace_nowrap()
                    .text_color(if segment.warn { warning } else { muted })
                    .child(SharedString::from(segment.text))
            }));
        v_flex()
            .gap_1()
            .child(strip)
            .child(v_flex().child(ruler).child(position))
            .child(readout_line)
            .into_any_element()
    }
}

impl Render for WaveformStrip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(analyzer) = self.strip.analyzer() else {
            if self.strip.path().is_none() {
                return div().into_any_element();
            }
            return self.idle(cx);
        };
        let snapshot = analyzer.snapshot();
        if let State::Failed(_) = snapshot.state {
            return self.muted_line("waveform-error", readout(&snapshot).text(), cx);
        }
        let focused = self.focus.is_focused(window);
        self.drawn(&snapshot, focused, cx)
    }
}
