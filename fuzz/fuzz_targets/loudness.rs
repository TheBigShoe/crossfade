#![no_main]
//! The meter's promise: whatever rate, roles, chunking and sample values (NaN and infinities
//! included) it never panics, never reports NaN, keeps its heap inside the documented bound
//! (16 bytes per 100 ms of programme), never reads a true peak under the sample peak, and gives
//! the same bits for any chunking.

use arbitrary::Arbitrary;
use crossfade_loudness::{ChannelRole, Measurements, Meter, Options};
use libfuzzer_sys::fuzz_target;

#[derive(Debug, Arbitrary)]
struct Input {
    rate: u32,
    rate_mode: u8,
    roles: Vec<u8>,
    true_peak: bool,
    loudness_range: bool,
    /// Squeeze finite samples to a sane level, so gates and ranges are reached too.
    tame: bool,
    repeat: u8,
    chunks: Vec<u16>,
    samples: Vec<f64>,
}

const ROLES: [ChannelRole; 8] = [
    ChannelRole::Left,
    ChannelRole::Right,
    ChannelRole::Centre,
    ChannelRole::LeftSurround,
    ChannelRole::RightSurround,
    ChannelRole::Lfe,
    ChannelRole::Unweighted,
    ChannelRole::Excluded,
];
const MAX_SAMPLES: usize = 1 << 18;

fn finite(v: Option<f64>) {
    assert!(v.is_none_or(f64::is_finite), "{v:?}");
}

fn check(r: &Measurements) {
    for v in [
        r.integrated,
        r.momentary,
        r.momentary_max,
        r.short_term,
        r.short_term_max,
        r.loudness_range,
        r.true_peak,
    ] {
        finite(v);
    }
    assert!(r.sample_peak.is_finite() && r.sample_peak >= 0.0);
    if let Some(tp) = r.true_peak {
        assert!(tp >= r.sample_peak);
    }
}

fuzz_target!(|input: Input| {
    // Mostly legal rates, sometimes anything at all.
    let rate = match input.rate_mode % 3 {
        0 => input.rate,
        1 => 8_000 + input.rate % 40_001,
        _ => 8_000 + input.rate % (1_048_575 - 8_000 + 1),
    };
    let roles: Vec<ChannelRole> = input.roles.iter().map(|&b| ROLES[usize::from(b) % 8]).collect();
    let options = Options {
        true_peak: input.true_peak,
        loudness_range: input.loudness_range,
    };
    let Ok(mut meter) = Meter::new(rate, &roles, options) else {
        assert!(
            !(8_000..=1_048_575).contains(&rate) || roles.is_empty() || roles.len() > 64,
            "a valid configuration was refused"
        );
        return;
    };

    let mut samples = Vec::new();
    for _ in 0..=usize::from(input.repeat % 32) {
        if samples.len() + input.samples.len() > MAX_SAMPLES {
            break;
        }
        samples.extend(input.samples.iter().map(|&s| {
            if input.tame && s.is_finite() {
                s % 2.0
            } else {
                s
            }
        }));
    }

    let mut at = 0;
    for (i, &c) in input.chunks.iter().enumerate() {
        let end = (at + usize::from(c)).min(samples.len());
        meter.push(&samples[at..end]);
        at = end;
        if i % 7 == 0 {
            check(&meter.measurements());
        }
    }
    meter.push(&samples[at..]);
    let chunked = meter.measurements();
    check(&chunked);

    let frames = samples.len() / roles.len();
    assert_eq!(meter.frames(), frames as u64);
    // 16 bytes per 100 ms, doubled for Vec growth, plus a fixed allowance per channel.
    let tenths = frames as u64 * 10 / u64::from(rate) + 2;
    let bound = 32 * tenths as usize + 4096 + 512 * roles.len();
    assert!(meter.heap_bytes() <= bound, "{} > {bound}", meter.heap_bytes());

    let mut whole = Meter::new(rate, &roles, options).unwrap();
    whole.push(&samples);
    assert_eq!(whole.measurements(), chunked, "chunking changed a result");
});
