//! Construction, errors, the length rule and equal-rate passthrough.
use crossfade_resample::{Error, MAX_CHANNELS, MAX_RATE, Quality, Resampler, output_len};

#[test]
fn zero_or_out_of_range_rates_and_channels_are_errors() {
    let q = Quality::Balanced;
    assert_eq!(
        Resampler::new(0, 48_000, 2, q).err(),
        Some(Error::InvalidRate(0))
    );
    assert_eq!(
        Resampler::new(44_100, 0, 2, q).err(),
        Some(Error::InvalidRate(0))
    );
    assert_eq!(
        Resampler::new(MAX_RATE + 1, 48_000, 2, q).err(),
        Some(Error::InvalidRate(MAX_RATE + 1))
    );
    assert_eq!(
        Resampler::new(44_100, 48_000, 0, q).err(),
        Some(Error::InvalidChannels(0))
    );
    assert_eq!(
        Resampler::new(44_100, 48_000, MAX_CHANNELS + 1, q).err(),
        Some(Error::InvalidChannels(MAX_CHANNELS + 1))
    );
    assert!(Resampler::new(1, MAX_RATE, MAX_CHANNELS, q).is_ok());
}

#[test]
fn errors_have_readable_texts() {
    assert_eq!(
        Error::InvalidRate(0).to_string(),
        "sample rate 0 Hz is outside 1 to 1048575 Hz"
    );
    assert_eq!(
        Error::InvalidChannels(0).to_string(),
        "channel count 0 is outside 1 to 256"
    );
    assert_eq!(
        Error::PartialFrame {
            samples: 3,
            channels: 2
        }
        .to_string(),
        "3 samples are not whole frames of 2 channels"
    );
    assert_eq!(
        Error::ChannelMismatch {
            expected: 2,
            got: 1
        }
        .to_string(),
        "planar call has 1 channels, the resampler 2"
    );
    assert_eq!(
        Error::UnequalLengths.to_string(),
        "planar input channels differ in length"
    );
}

#[test]
fn balanced_is_the_default_quality() {
    assert_eq!(Quality::default(), Quality::Balanced);
}

#[test]
fn accessors_report_the_configuration() {
    let r = Resampler::new(44_100, 48_000, 2, Quality::Fast).unwrap();
    assert_eq!(r.input_rate(), 44_100);
    assert_eq!(r.output_rate(), 48_000);
    assert_eq!(r.channels(), 2);
    assert_eq!(r.quality(), Quality::Fast);
}

#[test]
fn output_length_rounds_up() {
    assert_eq!(output_len(0, 44_100, 48_000), 0);
    assert_eq!(output_len(1, 44_100, 48_000), 2);
    assert_eq!(output_len(147, 44_100, 48_000), 160);
    assert_eq!(output_len(148, 44_100, 48_000), 162);
    assert_eq!(output_len(160, 48_000, 44_100), 147);
    assert_eq!(output_len(161, 48_000, 44_100), 148);
    assert_eq!(output_len(1_048_575, 1_048_575, 1), 1);
    assert_eq!(output_len(1_048_576, 1_048_575, 1), 2);
    assert_eq!(output_len(u64::MAX, 1, 1), u64::MAX);
}

#[test]
fn equal_rates_pass_samples_through_bit_for_bit() {
    let input: Vec<f64> = [0.5, -0.0, f64::MIN_POSITIVE / 4.0, 1e300, -1.0, 0.25]
        .into_iter()
        .cycle()
        .take(600)
        .collect();
    let mut r = Resampler::new(48_000, 48_000, 3, Quality::Best).unwrap();
    assert_eq!(r.latency(), 0);
    let mut out = Vec::new();
    for chunk in input.chunks(9) {
        r.process(&chunk[..chunk.len() / 3 * 3], &mut out).unwrap();
    }
    r.flush(&mut out);
    let bits = |v: &[f64]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&out), bits(&input));
}

#[test]
fn partial_frames_are_refused_without_consuming_anything() {
    let mut r = Resampler::new(44_100, 48_000, 2, Quality::Fast).unwrap();
    let mut out = Vec::new();
    assert_eq!(
        r.process(&[1.0, 2.0, 3.0], &mut out),
        Err(Error::PartialFrame {
            samples: 3,
            channels: 2
        })
    );
    r.flush(&mut out);
    assert!(out.is_empty());
}

#[test]
fn planar_calls_check_their_shape() {
    let mut r = Resampler::new(44_100, 48_000, 2, Quality::Fast).unwrap();
    let mut out = vec![Vec::new(), Vec::new()];
    assert_eq!(
        r.process_planar(&[&[1.0][..]], &mut out),
        Err(Error::ChannelMismatch {
            expected: 2,
            got: 1
        })
    );
    assert_eq!(
        r.process_planar(&[&[1.0][..], &[1.0, 2.0][..]], &mut out),
        Err(Error::UnequalLengths)
    );
    assert_eq!(
        r.flush_planar(&mut out[..1]),
        Err(Error::ChannelMismatch {
            expected: 2,
            got: 1
        })
    );
}
