use crossfade_app::waveform::{BASE, Pyramid};
use crossfade_core::{AudioFrame, AudioSpec, ChannelLayout, SampleFormat, Samples};
use proptest::prelude::*;

fn frame(channels: u16, interleaved: &[f64]) -> AudioFrame {
    let layout = ChannelLayout::default_for(channels).unwrap();
    let spec = AudioSpec::new(48_000, SampleFormat::F64, layout).unwrap();
    let mut f = AudioFrame::new(&spec);
    f.samples = Samples::F64(interleaved.to_vec());
    f
}

fn push_chunked(p: &mut Pyramid, channels: u16, samples: &[f64], cuts: &[usize]) {
    let per = usize::from(channels);
    let frames = samples.len() / per;
    let mut at = 0;
    for &c in cuts {
        let to = (at + c).min(frames);
        p.push(&frame(channels, &samples[at * per..to * per]))
            .unwrap();
        at = to;
    }
    p.push(&frame(channels, &samples[at * per..])).unwrap();
}

fn minmax(it: impl Iterator<Item = f64>) -> Option<(f32, f32)> {
    it.fold(None, |a, v| {
        let v = v as f32;
        Some(a.map_or((v, v), |(lo, hi)| (lo.min(v), hi.max(v))))
    })
}

/// What a column must hold: every block of the chosen level that it touches, whole.
fn oracle(
    samples: &[f64],
    channels: usize,
    ch: usize,
    p: &Pyramid,
    start: u64,
    fpc: u64,
    i: u64,
) -> Option<(f32, f32)> {
    let len = (samples.len() / channels) as u64;
    let s = start + i * fpc;
    if s >= len {
        return None;
    }
    let e = (s + fpc.max(1)).min(len);
    let b = p.block_frames(p.level_for(fpc));
    let (from, to) = (s / b * b, ((e - 1) / b + 1) * b);
    minmax((from..to.min(len)).map(|f| samples[f as usize * channels + ch]))
}

fn signal() -> impl Strategy<Value = (u16, Vec<f64>)> {
    (1u16..4, 0usize..6000).prop_flat_map(|(c, n)| {
        (
            Just(c),
            prop::collection::vec(-1.0f64..1.0, n * usize::from(c)),
        )
    })
}

proptest! {
    #[test]
    fn columns_equal_a_brute_force_scan(
        (channels, samples) in signal(),
        cuts in prop::collection::vec(0usize..2000, 0..6),
        fpc in 1u64..5000,
        start in 0u64..7000,
        n in 0usize..40,
    ) {
        let mut p = Pyramid::new(channels);
        push_chunked(&mut p, channels, &samples, &cuts);
        for ch in 0..usize::from(channels) {
            let got = p.columns(ch, start, fpc, n);
            prop_assert_eq!(got.len(), n);
            for (i, g) in got.iter().enumerate() {
                let want = oracle(&samples, usize::from(channels), ch, &p, start, fpc, i as u64);
                prop_assert_eq!(*g, want);
            }
        }
    }

    #[test]
    fn aligned_columns_are_exact(
        samples in prop::collection::vec(-1.0f64..1.0, 0..9000),
        shift in 0u32..4,
        first in 0u64..4,
    ) {
        let mut p = Pyramid::new(1);
        push_chunked(&mut p, 1, &samples, &[777]);
        let fpc = BASE << shift;
        let start = first * fpc;
        for (i, c) in p.columns(0, start, fpc, 6).iter().enumerate() {
            let s = (start + i as u64 * fpc) as usize;
            let e = (s + fpc as usize).min(samples.len());
            let want = if s < samples.len() { minmax(samples[s..e].iter().copied()) } else { None };
            prop_assert_eq!(*c, want);
        }
    }

    #[test]
    fn building_incrementally_equals_building_at_once(
        (channels, samples) in signal(),
        cuts in prop::collection::vec(1usize..1500, 1..8),
    ) {
        let mut inc = Pyramid::new(channels);
        let per = usize::from(channels);
        let mut at = 0;
        for c in cuts {
            let to = (at + c).min(samples.len() / per);
            inc.push(&frame(channels, &samples[at * per..to * per])).unwrap();
            at = to;
            // A partial pyramid is the pyramid of what has arrived.
            let mut whole = Pyramid::new(channels);
            whole.push(&frame(channels, &samples[..at * per])).unwrap();
            prop_assert_eq!(&inc, &whole);
        }
    }
}

#[test]
fn levels_halve_up_to_one_block_and_each_merges_its_children() {
    let samples: Vec<f64> = (0..100_000 * 2)
        .map(|i| f64::from((i * 7919) % 2001) / 1000.0 - 1.0)
        .collect();
    let mut p = Pyramid::new(2);
    push_chunked(&mut p, 2, &samples, &[12_345, 999]);
    assert_eq!(p.frames(), 100_000);
    assert_eq!(p.level_len(0), 100_000usize.div_ceil(BASE as usize));
    let top = p.levels() - 1;
    assert_eq!(p.level_len(top), 1);
    for k in 1..p.levels() {
        assert_eq!(p.level_len(k), p.level_len(k - 1).div_ceil(2));
        for ch in 0..2 {
            for j in 0..p.level_len(k) {
                let a = p.block(k - 1, ch, 2 * j).unwrap();
                let merged = match p.block(k - 1, ch, 2 * j + 1) {
                    Some(b) => (a.0.min(b.0), a.1.max(b.1)),
                    None => a,
                };
                assert_eq!(p.block(k, ch, j), Some(merged));
            }
        }
    }
}

#[test]
fn a_partial_last_block_counts_and_columns_past_the_end_are_none() {
    let mut p = Pyramid::new(1);
    let mut s = vec![0.0; 300];
    s[299] = 0.5;
    s[10] = -0.25;
    p.push(&frame(1, &s)).unwrap();
    assert_eq!(p.level_len(0), 2);
    assert_eq!(p.block(0, 0, 1), Some((0.0, 0.5)));
    assert_eq!(
        p.columns(0, 0, BASE, 3),
        vec![Some((-0.25, 0.0)), Some((0.0, 0.5)), None]
    );
    assert_eq!(p.columns(0, 10_000, 1, 2), vec![None, None]);
    assert_eq!(p.columns(5, 0, 1, 2), vec![None, None], "no such channel");
    assert_eq!(Pyramid::new(1).columns(0, 0, 1, 2), vec![None, None]);
}

#[test]
fn the_union_spans_all_channels() {
    let mut p = Pyramid::new(2);
    p.push(&frame(2, &[0.5, -0.5, 0.25, 0.75])).unwrap();
    assert_eq!(p.columns_union(0, 4, 1), vec![Some((-0.5, 0.75))]);
}

#[test]
fn nan_counts_as_silence_and_infinity_saturates() {
    let mut p = Pyramid::new(1);
    p.push(&frame(1, &[f64::NAN, f64::INFINITY, -1e300]))
        .unwrap();
    let (lo, hi) = p.columns(0, 0, 3, 1)[0].unwrap();
    assert_eq!((lo, hi), (f32::NEG_INFINITY, f32::INFINITY));
    let mut q = Pyramid::new(1);
    q.push(&frame(1, &[f64::NAN])).unwrap();
    assert_eq!(q.columns(0, 0, 1, 1), vec![Some((0.0, 0.0))]);
}

#[test]
fn a_frame_that_does_not_fit_is_refused_and_changes_nothing() {
    let mut p = Pyramid::new(2);
    assert!(p.push(&frame(1, &[0.1, 0.2])).is_err());
    let mut f = frame(2, &[0.0, 0.0]);
    f.samples = Samples::S16(vec![1, 2]);
    assert!(p.push(&f).is_err());
    assert_eq!(p.frames(), 0);
}

#[test]
fn one_hour_of_48k_stereo_fits_in_22_mb_and_a_query_costs_its_columns() {
    let mut p = Pyramid::new(2);
    // A minute of a constant, pushed sixty times: computed, not decoded.
    let minute = frame(2, &vec![0.25; 48_000 * 60 * 2]);
    for _ in 0..60 {
        p.push(&minute).unwrap();
    }
    let hour = 48_000 * 3600;
    assert_eq!(p.frames(), hour);
    assert_eq!(p.level_len(0), 675_000);
    assert_eq!(p.heap_bytes_level(0), 675_000 * 2 * 8);
    assert!(p.heap_bytes() < 22_000_000, "{}", p.heap_bytes());
    // Work is proportional to the column count, not to the range it spans.
    for (fpc, n) in [
        (1u64, 1000usize),
        (BASE, 1000),
        (172_800, 1000),
        (hour, 1000),
    ] {
        let (cols, reads) = p.columns_counting(0, 0, fpc, n);
        assert_eq!(cols.len(), n);
        assert!(
            reads <= 4 * n,
            "{reads} reads for {n} columns of {fpc} frames"
        );
    }
    let (_, whole_hour) = p.columns_counting(0, 0, hour / 100, 100);
    assert!(whole_hour <= 400);
}
