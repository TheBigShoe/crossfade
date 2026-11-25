use super::*;

/// A pulse train of `period` samples through a one-pole lowpass: voiced-speech-like.
fn pulse_train(len: usize, period: usize, amp: i32) -> Vec<i16> {
    let mut y = 0i32;
    (0..len)
        .map(|n| {
            let x = if n % period == 0 { amp } else { 0 };
            y = x + (y * 7) / 8;
            y.clamp(-32768, 32767) as i16
        })
        .collect()
}

fn noise(len: usize, seed: u32, amp: i32) -> Vec<i16> {
    let mut s = seed | 1;
    (0..len)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            ((s >> 16) as i32 % amp) as i16
        })
        .collect()
}

#[test]
fn a_pulse_train_is_voiced_at_its_period() {
    for fs_khz in [8usize, 12, 16] {
        for nb_subfr in [2usize, 4] {
            for complexity in 0..3 {
                // 125 Hz: a lag of 8 ms.
                let period = 8 * fs_khz;
                let len = (20 + 5 * nb_subfr) * fs_khz;
                let x = pulse_train(len, period, 8000);
                let mut corr = 0;
                let p = pitch_analysis_core(
                    &x, &mut corr, 0, 52429, 5000, fs_khz, complexity, nb_subfr,
                )
                .unwrap_or_else(|| panic!("{fs_khz} kHz {nb_subfr} subframes: unvoiced"));
                for &l in &p.lags[..nb_subfr] {
                    assert!(
                        (l - period as i32).abs() <= 1,
                        "{fs_khz} kHz: lag {l}, want {period}"
                    );
                }
                assert!(corr > 16384, "{corr}");
            }
        }
    }
}

#[test]
fn silence_and_noise_are_unvoiced() {
    for fs_khz in [8usize, 16] {
        let len = 40 * fs_khz;
        let mut corr = 12345;
        let silent = vec![0i16; len];
        assert_eq!(
            pitch_analysis_core(&silent, &mut corr, 0, 52429, 5000, fs_khz, 2, 4),
            None
        );
        assert_eq!(corr, 0);
        let n = noise(len, 99, 10000);
        assert_eq!(
            pitch_analysis_core(&n, &mut corr, 0, 52429, 5000, fs_khz, 2, 4),
            None
        );
    }
}

#[test]
fn the_downsamplers_keep_dc() {
    let x = vec![1000i16; 480];
    let mut out = vec![0i16; 240];
    let mut s = [0i32; 2];
    resampler_down2(&mut s, &mut out, &x);
    assert!((i32::from(out[200]) - 1000).abs() < 20, "{}", out[200]);
    let mut out = vec![0i16; 320];
    let mut s = [0i32; 6];
    resampler_down2_3(&mut s, &mut out, &x);
    assert!((i32::from(out[300]) - 1000).abs() < 40, "{}", out[300]);
}
