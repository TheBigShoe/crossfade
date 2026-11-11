use crossfade_app::analysis::{Loudness, Snapshot, State};
use crossfade_app::waveform::Pyramid;
use crossfade_app::waveview::{LanePlan, WaveView, readout};
use crossfade_core::{
    AudioFrame, AudioSpec, ChannelLayout, CodecId, SampleFormat, Samples, StreamInfo, TimeBase,
};

const RATE: u32 = 1000;

fn view(frames: u64, width: u32) -> WaveView {
    let mut v = WaveView::new();
    v.set_width(width);
    v.set_source(frames, RATE);
    v
}

#[test]
fn a_new_source_is_fitted() {
    let v = view(10_000, 100);
    assert!(v.is_fit());
    assert_eq!(v.frames_per_pixel(), 100);
    assert_eq!(v.visible(), (0, 10_000));
    // Not a multiple of the width: the last pixel is partly empty, never cut off.
    let v = view(10_050, 100);
    assert_eq!(v.frames_per_pixel(), 101);
    assert_eq!(v.visible(), (0, 10_050));
}

#[test]
fn columns_stop_where_the_file_does() {
    assert_eq!(view(10_000, 100).columns(), 100);
    assert_eq!(view(40, 100).columns(), 40);
    let mut v = view(10_000, 100);
    v.zoom(4.0, 0.0);
    v.pan_px(1e6);
    assert_eq!(v.columns(), 100);
}

#[test]
fn a_file_shorter_than_the_strip_is_one_frame_per_pixel() {
    let v = view(40, 100);
    assert_eq!(v.frames_per_pixel(), 1);
    assert!(v.is_fit());
}

#[test]
fn zoom_keeps_the_frame_under_the_pointer() {
    let mut v = view(10_000, 100);
    v.zoom(2.0, 50.0);
    assert_eq!(v.frames_per_pixel(), 50);
    assert_eq!(v.frame_at(50.0), 5_000);
    assert!(!v.is_fit());
    v.zoom(2.0, 0.0);
    assert_eq!(v.frames_per_pixel(), 25);
    assert_eq!(v.start(), 2_500);
}

#[test]
fn zoom_stops_at_one_frame_per_pixel_and_at_the_whole_file() {
    let mut v = view(10_000, 100);
    for _ in 0..40 {
        v.zoom(2.0, 10.0);
    }
    assert_eq!(v.frames_per_pixel(), 1);
    for _ in 0..40 {
        v.zoom(0.5, 10.0);
    }
    assert!(v.is_fit());
    assert_eq!(v.start(), 0);
}

#[test]
fn a_small_zoom_still_moves_at_low_frames_per_pixel() {
    let mut v = view(300, 100);
    assert_eq!(v.frames_per_pixel(), 3);
    v.zoom(1.05, 0.0);
    assert_eq!(v.frames_per_pixel(), 2);
    v.zoom(0.95, 0.0);
    assert_eq!(v.frames_per_pixel(), 3);
}

#[test]
fn pan_moves_by_pixels_and_clamps_at_both_ends() {
    let mut v = view(10_000, 100);
    v.zoom(4.0, 0.0);
    assert_eq!(v.frames_per_pixel(), 25);
    v.pan_px(10.0);
    assert_eq!(v.start(), 250);
    v.pan_px(-1000.0);
    assert_eq!(v.start(), 0);
    v.pan_px(100_000.0);
    assert_eq!(v.visible().1, 10_000);
    assert_eq!(v.start(), 10_000 - 25 * 100);
}

#[test]
fn panning_a_fitted_view_does_nothing() {
    let mut v = view(10_000, 100);
    v.pan_px(30.0);
    assert_eq!(v.start(), 0);
    assert!(v.is_fit());
}

#[test]
fn key_steps_zoom_about_the_centre_and_pan_a_tenth() {
    let mut v = view(10_000, 100);
    v.zoom_in();
    assert_eq!(v.frames_per_pixel(), 50);
    assert_eq!(v.visible(), (2_500, 7_500));
    v.pan_step(1);
    assert_eq!(v.start(), 3_000);
    v.pan_step(-1);
    v.zoom_out();
    assert!(v.is_fit());
}

#[test]
fn pinch_and_scroll_zoom_follow_their_direction() {
    let mut v = view(10_000, 100);
    v.pinch(0.5, 50.0);
    assert!(v.frames_per_pixel() < 100);
    let z = v.frames_per_pixel();
    v.pinch(-0.3, 50.0);
    assert!(v.frames_per_pixel() > z);
    let z = v.frames_per_pixel();
    v.zoom_scroll(-40.0, 50.0);
    assert!(v.frames_per_pixel() > z, "scrolling down zooms out");
    let z = v.frames_per_pixel();
    v.zoom_scroll(40.0, 50.0);
    assert!(v.frames_per_pixel() < z);
}

#[test]
fn resizing_keeps_a_fitted_view_fitted_and_clamps_a_zoomed_one() {
    let mut v = view(10_000, 100);
    v.set_width(200);
    assert!(v.is_fit());
    assert_eq!(v.frames_per_pixel(), 50);
    v.zoom(4.0, 0.0);
    v.pan_px(1e6);
    v.set_width(400);
    assert!(v.visible().1 <= 10_000);
}

#[test]
fn pixels_and_frames_map_both_ways() {
    let mut v = view(10_000, 100);
    v.zoom(2.0, 0.0);
    v.pan_px(20.0);
    assert_eq!(v.start(), 1_000);
    assert_eq!(v.frame_at(10.0), 1_500);
    assert!((v.x_of(1_500) - 10.0).abs() < 1e-3);
}

#[test]
fn a_growing_source_stays_fitted() {
    let mut v = view(1_000, 100);
    v.set_source(2_000, RATE);
    assert!(v.is_fit());
    assert_eq!(v.frames_per_pixel(), 20);
}

fn labels(v: &WaveView) -> Vec<String> {
    v.ticks().into_iter().map(|t| t.label).collect()
}

#[test]
fn ticks_use_nice_steps_and_minutes_seconds() {
    // 100 s over 600 px: 6 px a second.
    let v = view(100_000, 600);
    let ticks = v.ticks();
    assert_eq!(ticks[0].x, 0.0);
    assert_eq!(
        labels(&v),
        ["0:00", "0:15", "0:30", "0:45", "1:00", "1:15", "1:30"]
    );
    assert!((ticks[1].x - 90.0).abs() < 0.5);
}

#[test]
fn ticks_never_crowd() {
    let v = view(3_600_000, 300);
    let t = v.ticks();
    assert!(t.windows(2).all(|w| w[1].x - w[0].x >= 60.0));
    assert!(t.last().unwrap().x <= 300.0);
}

#[test]
fn an_hour_long_range_labels_hours() {
    let v = view(7_300_000, 600);
    let l = labels(&v);
    assert_eq!(l[0], "0:00:00");
    assert!(l.iter().any(|s| s.starts_with("1:")), "{l:?}");
}

#[test]
fn zoomed_in_ticks_gain_decimals_and_follow_the_range() {
    let mut v = view(10_000, 100);
    v.zoom(4.0, 0.0);
    v.zoom(4.0, 0.0);
    // 6 frames per pixel at 1 kHz, from 0.24 s: the half-second mark is the only one.
    v.pan_px(40.0);
    assert_eq!(v.frames_per_pixel(), 6);
    assert_eq!(labels(&v), ["0:00.5"]);
}

#[test]
fn the_position_bar_shows_only_when_zoomed() {
    let mut v = view(10_000, 100);
    assert_eq!(v.position(), None);
    v.zoom(4.0, 0.0);
    assert_eq!(v.position(), Some((0.0, 0.25)));
    v.pan_px(100.0);
    let (a, b) = v.position().unwrap();
    assert!((a - 0.25).abs() < 1e-6 && (b - 0.5).abs() < 1e-6);
}

#[test]
fn mono_and_stereo_draw_a_lane_each_and_more_draw_one() {
    assert_eq!(LanePlan::for_channels(1), LanePlan::Separate(1));
    assert_eq!(LanePlan::for_channels(2), LanePlan::Separate(2));
    assert_eq!(LanePlan::for_channels(3), LanePlan::Combined);
    assert_eq!(LanePlan::for_channels(8), LanePlan::Combined);
    assert_eq!(LanePlan::Separate(2).lanes(), 2);
    assert_eq!(LanePlan::Combined.lanes(), 1);
}

fn pyramid(channels: u16, frames: usize) -> Pyramid {
    let mut p = Pyramid::new(channels);
    let layout = ChannelLayout::default_for(channels).unwrap();
    let spec = AudioSpec::new(48_000, SampleFormat::F64, layout).unwrap();
    let samples: Vec<f64> = (0..frames * usize::from(channels))
        .map(|i| (i / usize::from(channels)) as f64 / frames as f64)
        .collect();
    let mut f = AudioFrame::new(&spec);
    f.samples = Samples::F64(samples);
    p.push(&f).unwrap();
    p
}

#[test]
fn draw_columns_give_at_most_one_column_per_pixel_per_lane() {
    let p = pyramid(2, 50_000);
    let mut v = WaveView::new();
    v.set_width(200);
    v.set_source(50_000, 48_000);
    let lanes = v.draw_columns(&p);
    assert_eq!(lanes.len(), 2);
    assert!(lanes.iter().all(|l| l.len() == 200));
    assert!(lanes[0].iter().all(Option::is_some));
    // Rising ramp: later columns are higher.
    assert!(lanes[0][199].unwrap().1 > lanes[0][10].unwrap().1);
    // Surround files draw the union in one lane.
    let p = pyramid(6, 50_000);
    assert_eq!(v.draw_columns(&p).len(), 1);
}

#[test]
fn unanalysed_columns_are_none_and_a_zoomed_view_is_still_one_per_pixel() {
    let p = pyramid(1, 25_000);
    let mut v = WaveView::new();
    v.set_width(100);
    v.set_source(50_000, 48_000);
    let lanes = v.draw_columns(&p);
    assert_eq!(lanes[0].len(), 100);
    assert!(lanes[0][10].is_some() && lanes[0][90].is_none());
    v.zoom(8.0, 0.0);
    assert_eq!(v.draw_columns(&p)[0].len(), 100);
}

fn snap(state: State, done: u64, total: Option<u64>, l: Loudness) -> Snapshot {
    Snapshot {
        version: 1,
        stream: total.map(stream_with),
        frames_done: done,
        state,
        loudness: l,
    }
}

fn stream_with(frames: u64) -> StreamInfo {
    let spec = AudioSpec::new(48_000, SampleFormat::S16, ChannelLayout::stereo()).unwrap();
    let mut s = StreamInfo::new(
        0,
        CodecId::PcmS16Le,
        spec,
        TimeBase::new(1, 48_000).unwrap(),
    );
    s.frames = Some(frames);
    s
}

fn loud(i: Option<f64>, r: Option<f64>, tp: Option<f64>, sp: f64) -> Loudness {
    Loudness {
        integrated: i,
        range: r,
        true_peak: tp,
        sample_peak: sp,
    }
}

#[test]
fn the_readout_line_reads_lufs_range_and_true_peak() {
    let r = readout(&snap(
        State::Done,
        100,
        Some(100),
        loud(Some(-16.24), Some(7.4), Some(0.85), 0.8),
    ));
    assert_eq!(
        r.text(),
        "\u{2212}16.2 LUFS \u{b7} 7.4 LU range \u{b7} \u{2212}1.4 dBTP"
    );
    assert!(!r.warns());
}

#[test]
fn a_true_peak_above_minus_one_warns_on_that_figure_only() {
    let r = readout(&snap(
        State::Done,
        100,
        Some(100),
        loud(Some(-9.0), Some(3.0), Some(1.0), 1.0),
    ));
    assert!(r.warns());
    let warned: Vec<_> = r.segments.iter().filter(|s| s.warn).collect();
    assert_eq!(warned.len(), 1);
    assert_eq!(warned[0].text, "0.0 dBTP");
}

#[test]
fn missing_figures_are_left_out_and_silence_says_so() {
    let r = readout(&snap(
        State::Done,
        100,
        Some(100),
        loud(Some(-20.0), None, Some(0.5), 0.5),
    ));
    assert_eq!(r.text(), "\u{2212}20.0 LUFS \u{b7} \u{2212}6.0 dBTP");
    let r = readout(&snap(
        State::Done,
        100,
        Some(100),
        loud(None, None, None, 0.0),
    ));
    assert_eq!(r.text(), "Silent");
    let r = readout(&snap(
        State::Done,
        100,
        Some(100),
        loud(None, None, None, 0.3),
    ));
    assert_eq!(r.text(), "Too short to measure");
}

#[test]
fn analysing_shows_a_percentage_when_the_length_is_known() {
    let r = readout(&snap(State::Running, 42, Some(100), Loudness::default()));
    assert_eq!(r.text(), "Analysing\u{2026} 42%");
    let r = readout(&snap(State::Running, 42, None, Loudness::default()));
    assert_eq!(r.text(), "Analysing\u{2026}");
    let r = readout(&snap(State::Running, 0, Some(0), Loudness::default()));
    assert_eq!(r.text(), "Analysing\u{2026}");
}

#[test]
fn failure_is_one_muted_line() {
    let r = readout(&snap(
        State::Failed("not a wave file".into()),
        0,
        None,
        Loudness::default(),
    ));
    assert_eq!(r.text(), "Could not analyse: not a wave file");
    assert!(!r.warns());
}
