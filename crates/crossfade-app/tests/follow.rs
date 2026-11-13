//! The waveform following the playhead while playing, and giving way to the user's own
//! scrolling until the next play or seek.

use crossfade_app::player::Follow;
use crossfade_app::waveview::WaveView;

/// 100 seconds at 1 kHz on 500 px, zoomed in to 10 frames per pixel (5,000 frames in view).
fn zoomed() -> WaveView {
    let mut v = WaveView::new();
    v.set_width(500);
    v.set_source(100_000, 1000);
    v.zoom(20.0, 0.0);
    assert_eq!(v.frames_per_pixel(), 10);
    assert_eq!(v.visible(), (0, 5000));
    v
}

#[test]
fn the_view_pages_to_keep_the_playhead_in_sight() {
    let follow = Follow::new();
    assert!(follow.is_following());
    let mut v = zoomed();
    assert!(!follow.keep_in_view(&mut v, 4999), "still in view");
    assert_eq!(v.start(), 0);
    assert!(follow.keep_in_view(&mut v, 5000));
    assert_eq!(v.visible(), (5000, 10_000), "a page on, the playhead at the left");
    assert_eq!(v.frames_per_pixel(), 10, "the zoom is the user's");
}

#[test]
fn a_playhead_before_the_view_brings_it_back() {
    let follow = Follow::new();
    let mut v = zoomed();
    v.pan_px(4000.0);
    assert_eq!(v.start(), 40_000);
    assert!(follow.keep_in_view(&mut v, 1234));
    assert_eq!(v.start(), 1234);
}

#[test]
fn the_last_page_stops_at_the_end_of_the_file() {
    let follow = Follow::new();
    let mut v = zoomed();
    assert!(follow.keep_in_view(&mut v, 98_000));
    assert_eq!(v.visible(), (95_000, 100_000));
}

#[test]
fn scrolling_stops_following_until_play_or_seek() {
    let mut follow = Follow::new();
    let mut v = zoomed();
    follow.user_moved();
    assert!(!follow.is_following());
    assert!(!follow.keep_in_view(&mut v, 50_000));
    assert_eq!(v.start(), 0);
    follow.resume();
    assert!(follow.keep_in_view(&mut v, 50_000));
    assert_eq!(v.start(), 50_000);
}

#[test]
fn a_fitted_view_never_moves() {
    let follow = Follow::new();
    let mut v = WaveView::new();
    v.set_width(500);
    v.set_source(100_000, 1000);
    assert!(!follow.keep_in_view(&mut v, 99_999));
    assert!(v.is_fit());
}
