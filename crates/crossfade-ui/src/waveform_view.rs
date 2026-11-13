//! The inspector's waveform strip and its transport. It renders `crossfade_app::strip::Strip`
//! and a `crossfade_app::player::Player`, and forwards gestures to their models; the arithmetic
//! lives there.
//!
//! The lanes are their own cached view ([`Lanes`]): they are asked for one column per
//! horizontal pixel per lane, and only when the analysis, the visible range, the width or the
//! colours change. While playing, the strip asks for a frame per display refresh to move the
//! playhead and the time; those frames reuse the lanes' painted quads as they are (no pyramid
//! query, no repaint), so a playing strip costs about what a blinking text cursor does: the
//! window's element tree is rebuilt and the strip's few small elements laid out again.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use crossfade_app::analysis::{Analyzer, Snapshot, State};
use crossfade_app::player::{Follow, Player, State as PlayState, transport_text};
use crossfade_app::strip::{Strip, Wake};
use crossfade_app::waveview::{Lane, LanePlan, WaveView, readout};
use crossfade_core::Limits;

use crate::actions::{
    WAVEFORM_CONTEXT, WaveformFit, WaveformPanLeft, WaveformPanRight, WaveformZoomIn,
    WaveformZoomOut,
};
use crate::chrome::{LABEL_SIZE, icon_button};
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
/// A press that moves further than this is a drag (a scrub), not a click (a seek).
const DRAG_PX: f32 = 3.;
/// What the strip says when there is no audio output.
const NO_OUTPUT: &str = "No audio output";

/// What the lanes were last computed for: the analysis, the range, the width and the colours.
#[derive(Clone, PartialEq)]
struct LanesKey {
    version: u64,
    start: u64,
    fpp: u64,
    width: u32,
    frames: u64,
    wave: Hsla,
    line: Hsla,
}

/// The painted lanes, a view of their own so a frame that only moves the playhead reuses them.
struct Lanes {
    key: Option<LanesKey>,
    lanes: Vec<Lane>,
    planned: usize,
    known: bool,
    columns: usize,
}

impl Render for Lanes {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let Some(key) = self.key.clone() else {
            return div().into_any_element();
        };
        let lanes = self.lanes.clone();
        let (planned, known, columns) = (self.planned, self.known, self.columns);
        canvas(
            |bounds, _, _| bounds,
            move |_, bounds, window, _| {
                let lane_count = if lanes.is_empty() {
                    planned
                } else {
                    lanes.len()
                };
                let width = f32::from(bounds.size.width);
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
                        key.line,
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
                            key.wave,
                        ));
                    }
                }
            },
        )
        .size_full()
        .into_any_element()
    }
}

pub struct WaveformStrip {
    strip: Strip,
    focus: FocusHandle,
    io: IoFactory,
    wake: Arc<Wake>,
    sender: async_channel::Sender<()>,
    /// Where the lanes are, in window pixels: their left edge, as last laid out.
    left: Rc<Cell<f32>>,
    lanes: Entity<Lanes>,
    /// `None` when there is no audio output; `audio_note` says why.
    player: Option<Player>,
    audio_note: Option<String>,
    /// The path the player has loaded.
    loaded: Option<String>,
    follow: Follow,
    /// Where the pointer went down on the lanes, strip pixels; set until it comes up.
    press: Option<f32>,
    _wakes: Task<()>,
}

impl WaveformStrip {
    pub fn new(io: IoFactory, audio: AudioOutput, cx: &mut Context<Self>) -> Self {
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
        let (player, audio_note) = match audio {
            AudioOutput::Sink(sink) => {
                let ended = sender.clone();
                // A load finishing and the end of playback, both rare: no coalescing needed.
                let made = Player::new(sink, io(), Limits::default(), move || {
                    let _ = ended.try_send(());
                });
                match made {
                    Ok(player) => (Some(player), None),
                    Err(why) => {
                        eprintln!("crossfade: playback off: {why}");
                        (None, Some(NO_OUTPUT.to_owned()))
                    }
                }
            }
            AudioOutput::Off(why) => (None, Some(why)),
        };
        Self {
            strip: Strip::default(),
            focus: cx.focus_handle(),
            io,
            wake,
            sender,
            left: Rc::new(Cell::new(0.)),
            lanes: cx.new(|_| Lanes {
                key: None,
                lanes: Vec::new(),
                planned: 1,
                known: false,
                columns: 0,
            }),
            player,
            audio_note,
            loaded: None,
            follow: Follow::new(),
            press: None,
            _wakes: wakes,
        }
    }

    pub fn path(&self) -> Option<&str> {
        self.strip.path()
    }

    pub fn view(&self) -> &WaveView {
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
    pub fn player_state(&self) -> Option<PlayState> {
        self.player.as_ref().map(Player::state)
    }

    /// The playback position in source frames.
    pub fn position(&self) -> Option<u64> {
        self.player.as_ref().map(Player::position)
    }

    /// Waits for the player's worker to catch up (tests and screenshots).
    pub fn settle(&self) {
        if let Some(player) = &self.player {
            player.settle();
        }
    }

    /// The player, for screenshots that set a scene up by hand.
    pub fn player_mut(&mut self) -> Option<&mut Player> {
        self.player.as_mut()
    }

    pub fn is_following(&self) -> bool {
        self.follow.is_following()
    }

    pub fn is_scrubbing(&self) -> bool {
        self.player.as_ref().is_some_and(Player::is_scrubbing)
    }

    /// The transport's time readout, when it shows one.
    pub fn transport_text(&self) -> Option<String> {
        let player = self.playable()?;
        let rate = player.stream().map_or(0, |s| s.audio.sample_rate());
        Some(transport_text(player.position(), player.duration(), rate))
    }

    /// The player, when the shown file has one to play (loading counts).
    fn playable(&self) -> Option<&Player> {
        let player = self.player.as_ref()?;
        self.strip.path()?;
        match player.state() {
            PlayState::Empty | PlayState::Failed(_) => None,
            _ => Some(player),
        }
    }

    pub fn toggle_playback(&mut self, cx: &mut Context<Self>) {
        if self.playable().is_none() {
            return;
        }
        if let Some(player) = &mut self.player {
            player.toggle();
            if player.state() == PlayState::Playing {
                self.follow.resume();
            }
        }
        cx.notify();
    }

    fn seek(&mut self, frame: u64, cx: &mut Context<Self>) {
        if self.playable().is_none() {
            return;
        }
        if let Some(player) = &mut self.player {
            player.seek(frame);
            self.follow.resume();
        }
        cx.notify();
    }

    pub fn to_start(&mut self, cx: &mut Context<Self>) {
        self.seek(0, cx);
    }

    pub fn to_end(&mut self, cx: &mut Context<Self>) {
        let end = self.player.as_ref().and_then(Player::duration);
        if let Some(end) = end {
            self.seek(end, cx);
        }
    }

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
        self.sync_player();
        cx.notify();
    }

    /// The player follows the selection: another file is loaded, stopped at its start.
    fn sync_player(&mut self) {
        let path = self.strip.path().map(str::to_owned);
        if path == self.loaded {
            return;
        }
        if let Some(player) = &mut self.player {
            match &path {
                Some(p) => player.load(p.as_str()),
                None => player.stop(),
            }
        }
        self.loaded = path;
        self.follow.resume();
    }

    fn analyse(&mut self, cx: &mut Context<Self>) {
        let start = self.starter();
        self.strip.analyse(start);
        cx.notify();
    }

    /// A move of the user's own: the view stops following the playhead.
    fn edit(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut WaveView)) {
        f(self.strip.view_mut());
        self.follow.user_moved();
        cx.notify();
    }

    fn pointer_x(&self, position: Point<Pixels>) -> f32 {
        let width = self.strip.view().width() as f32;
        (f32::from(position.x) - self.left.get()).clamp(0., width)
    }

    fn pointer_down(&mut self, x: f32) {
        if self.playable().is_some() {
            self.press = Some(x);
        }
    }

    fn pointer_moved(&mut self, x: f32, cx: &mut Context<Self>) {
        let Some(from) = self.press else {
            return;
        };
        let scrubbing = self.is_scrubbing();
        if !scrubbing && (x - from).abs() < DRAG_PX {
            return;
        }
        let frame = self.strip.view().frame_at(x);
        if let Some(player) = &mut self.player {
            player.scrub_to(frame);
        }
        cx.notify();
    }

    fn pointer_up(&mut self, x: f32, cx: &mut Context<Self>) {
        if self.press.take().is_none() {
            return;
        }
        if self.is_scrubbing() {
            self.pointer_moved(x, cx);
            if let Some(player) = &mut self.player {
                player.end_scrub();
            }
            self.follow.resume();
            cx.notify();
        } else {
            let frame = self.strip.view().frame_at(x);
            self.seek(frame, cx);
        }
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

    /// The play button and the time, or the reason there is none.
    fn transport(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let muted = cx.theme().muted_foreground;
        if let Some(note) = &self.audio_note {
            return Some(
                div()
                    .id("transport-note")
                    .test_support()
                    .flex_none()
                    .whitespace_nowrap()
                    .text_size(LABEL_SIZE)
                    .text_color(muted)
                    .child(SharedString::from(note.clone()))
                    .into_any_element(),
            );
        }
        let player = self.playable()?;
        let playing = player.state() == PlayState::Playing;
        let (icon, tip) = if playing {
            (IconName::Pause, "Pause")
        } else {
            (IconName::Play, "Play")
        };
        let time = self.transport_text().unwrap_or_default();
        Some(
            h_flex()
                .flex_none()
                .gap_1()
                .items_center()
                .child(
                    icon_button("transport-play", icon, tip, cx)
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_playback(cx))),
                )
                .child(
                    div()
                        .id("transport-time")
                        .test_support()
                        .whitespace_nowrap()
                        .text_size(LABEL_SIZE)
                        .text_color(muted)
                        .child(SharedString::from(time)),
                )
                .into_any_element(),
        )
    }

    fn idle(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, foreground) = (theme.muted_foreground, theme.foreground);
        let analyse = div()
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
            );
        v_flex()
            .gap_1()
            .child(analyse)
            .children(self.transport(cx))
            .into_any_element()
    }

    /// Keeps the lanes view's data current; recomputes the columns only when what they show
    /// changed.
    fn update_lanes(&mut self, snapshot: &Snapshot, view: &WaveView, cx: &mut Context<Self>) {
        let theme = cx.theme();
        let channels = snapshot
            .stream
            .as_ref()
            .map_or(2, |s| usize::from(s.audio.layout().channels()));
        let key = LanesKey {
            version: snapshot.version,
            start: view.start(),
            fpp: view.frames_per_pixel(),
            width: view.width(),
            frames: view.frames(),
            wave: theme.primary.opacity(0.6),
            line: theme.muted_foreground.opacity(0.35),
        };
        if self.lanes.read(cx).key.as_ref() == Some(&key) {
            return;
        }
        let lanes: Vec<Lane> = self
            .strip
            .analyzer()
            .map(|a| a.with(|analysis| view.draw_columns(&analysis.pyramid)))
            .unwrap_or_default();
        let planned = LanePlan::for_channels(channels).lanes().max(1);
        let (known, columns) = (view.frames() > 0, view.columns());
        self.lanes.update(cx, |l, cx| {
            *l = Lanes {
                key: Some(key),
                lanes,
                planned,
                known,
                columns,
            };
            cx.notify();
        });
    }

    fn drawn(
        &mut self,
        snapshot: &Snapshot,
        focused: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let (wave, muted, warning, head) = (
            theme.primary.opacity(0.6),
            theme.muted_foreground,
            theme.warning,
            theme.foreground.opacity(0.85),
        );
        let (frames, rate) = match &snapshot.stream {
            Some(stream) => (
                stream.frames.unwrap_or(snapshot.frames_done),
                stream.audio.sample_rate(),
            ),
            None => (0, 0),
        };
        self.strip.view_mut().set_source(frames, rate);
        let playing = self
            .playable()
            .is_some_and(|p| p.state() == PlayState::Playing);
        let position = self.playable().map(Player::position);
        if let Some(at) = position.filter(|_| playing) {
            self.follow.keep_in_view(self.strip.view_mut(), at);
        }
        if playing || self.is_scrubbing() {
            window.request_animation_frame();
        }
        let view = self.strip.view().clone();
        self.update_lanes(snapshot, &view, cx);
        let left = Rc::clone(&self.left);
        let entity = cx.entity();
        // Measures the lanes' place without painting: the pointer maths and the width.
        let measure = canvas(
            |bounds, _, _| bounds,
            move |_, bounds, _, cx| {
                left.set(f32::from(bounds.origin.x));
                let w = f32::from(bounds.size.width).floor() as u32;
                entity.update(cx, |this, cx| {
                    if this.strip.view().width() != w {
                        this.strip.view_mut().set_width(w);
                        cx.notify();
                    }
                });
            },
        )
        .absolute()
        .size_full();
        let playhead = position
            .map(|at| view.x_of(at))
            .filter(|x| (0.0..=view.width() as f32).contains(x) && view.frames() > 0)
            .map(|x| {
                div()
                    .id("waveform-playhead")
                    .test_support()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(x.floor()))
                    .w(px(1.5))
                    .bg(head)
            });
        let background = self.lane_background(cx);
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
            .when(self.playable().is_some(), |s| s.cursor_pointer())
            .child(measure)
            .child(
                self.lanes
                    .clone()
                    .cached(StyleRefinement::default().absolute().size_full()),
            )
            .children(playhead)
            .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                window.focus(&this.focus, cx);
                if event.click_count() == 2 {
                    this.edit(cx, |view| view.fit());
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, _| {
                    let x = this.pointer_x(event.position);
                    this.pointer_down(x);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if event.pressed_button == Some(MouseButton::Left) {
                    let x = this.pointer_x(event.position);
                    this.pointer_moved(x, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _, cx| {
                    let x = this.pointer_x(event.position);
                    this.pointer_up(x, cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _, cx| {
                    let x = this.pointer_x(event.position);
                    this.pointer_up(x, cx);
                }),
            )
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

        let position_bar = div()
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
            .flex_1()
            .min_w_0()
            .justify_end()
            .overflow_hidden()
            .h(LINE_HEIGHT)
            .line_height(LINE_HEIGHT)
            .text_size(LABEL_SIZE)
            .children(line.segments.into_iter().map(|segment| {
                div()
                    .whitespace_nowrap()
                    .text_color(if segment.warn { warning } else { muted })
                    .child(SharedString::from(segment.text))
            }));
        let under = h_flex()
            .w_full()
            .gap_3()
            .items_center()
            .children(self.transport(cx))
            .child(readout_line);
        v_flex()
            .gap_1()
            .child(strip)
            .child(v_flex().child(ruler).child(position_bar))
            .child(under)
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
        self.drawn(&snapshot, focused, window, cx)
    }
}
