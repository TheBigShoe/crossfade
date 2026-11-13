use super::super::macros::sqrt_approx;
use super::super::tables_nlsf::{NLSF_CB_NB_MB, NLSF_CB_WB};
use super::*;

/// RFC 6716's decoder computes each first-stage vector's weights (Laroia's formula, then a
/// square root) where libopus reads them from `CB1_Wght_Q9`: they must agree, or a decoder
/// built from the tables would not be the RFC's.
#[test]
fn the_weight_tables_are_the_rfcs_laroia_weights() {
    for cb in [&NLSF_CB_NB_MB, &NLSF_CB_WB] {
        let d = cb.order;
        for v in 0..cb.n_vectors {
            let x: Vec<i32> = cb.cb1_nlsf_q8[v * d..][..d]
                .iter()
                .map(|&c| i32::from(c) << 7)
                .collect();
            let inv = |gap: i32| (1 << 17) / gap.max(1);
            let mut w = vec![0i32; d];
            let mut tmp2 = inv(x[1] - x[0]);
            w[0] = (inv(x[0]) + tmp2).min(32767);
            for k in (1..d - 1).step_by(2) {
                let tmp1 = inv(x[k + 1] - x[k]);
                w[k] = (tmp1 + tmp2).min(32767);
                tmp2 = inv(x[k + 2] - x[k + 1]);
                w[k + 1] = (tmp1 + tmp2).min(32767);
            }
            w[d - 1] = (inv((1 << 15) - x[d - 1]) + tmp2).min(32767);
            for k in 0..d {
                let q9 = sqrt_approx(w[k] << 16);
                assert_eq!(
                    i32::from(cb.cb1_wght_q9[v * d + k]),
                    q9,
                    "order {d} vector {v} coefficient {k}"
                );
            }
        }
    }
}

#[test]
fn icdf_tables_fall_and_end_in_zero() {
    for cb in [&NLSF_CB_NB_MB, &NLSF_CB_WB] {
        assert_eq!(cb.cb1_icdf.len(), 2 * cb.n_vectors);
        assert_eq!(cb.cb1_nlsf_q8.len(), cb.n_vectors * cb.order);
        assert_eq!(cb.cb1_wght_q9.len(), cb.n_vectors * cb.order);
        assert_eq!(cb.ec_sel.len(), cb.n_vectors * cb.order / 2);
        assert_eq!(cb.pred_q8.len(), 2 * (cb.order - 1));
        assert_eq!(cb.delta_min_q15.len(), cb.order + 1);
        assert!(cb.cb1_wght_q9.iter().all(|&w| w > 0));
        for half in cb.cb1_icdf.chunks(cb.n_vectors) {
            assert_eq!(*half.last().unwrap(), 0);
            assert!(half.windows(2).all(|p| p[0] >= p[1]));
        }
        // Nine-symbol tables for the second stage.
        for t in cb.ec_icdf.chunks(9) {
            assert_eq!(*t.last().unwrap(), 0);
            assert!(t.windows(2).all(|p| p[0] >= p[1]));
        }
    }
}

#[test]
fn stabilize_spaces_the_frequencies() {
    let delta = NLSF_CB_NB_MB.delta_min_q15;
    for start in [
        [0i16; 10],
        [32767; 10],
        [5000, 4000, 3000, 2000, 1000, 0, 0, 0, 0, 0],
    ] {
        let mut nlsf = start;
        nlsf_stabilize(&mut nlsf, delta);
        assert!(nlsf[0] >= delta[0], "{nlsf:?}");
        for i in 1..10 {
            assert!(
                i32::from(nlsf[i]) - i32::from(nlsf[i - 1]) >= i32::from(delta[i]),
                "{start:?} -> {nlsf:?}"
            );
        }
        assert!(i32::from(nlsf[9]) <= 32768 - i32::from(delta[10]));
    }
    let mut ok = [
        2000, 5000, 8000, 11000, 14000, 17000, 20000, 23000, 26000, 29000,
    ];
    let before = ok;
    nlsf_stabilize(&mut ok, delta);
    assert_eq!(ok, before);
}

#[test]
fn nlsf2a_gives_stable_filters() {
    let mut a = [0i16; 16];
    for d in [10usize, 16] {
        let even: Vec<i16> = (1..=d as i32)
            .map(|i| (i * 32768 / (d as i32 + 1)) as i16)
            .collect();
        nlsf2a(&mut a[..d], &even);
        assert!(lpc_inverse_pred_gain(&a[..d]) > 0, "{a:?}");
        // Evenly spread frequencies are white: the filter is (close to) zero.
        assert!(a[..d].iter().all(|&c| c.abs() < 64), "{a:?}");
        // Crowded frequencies give a resonant but still stable filter.
        let crowded: Vec<i16> = (0..d as i32).map(|i| (3000 + i * 60) as i16).collect();
        nlsf2a(&mut a[..d], &crowded);
        assert!(lpc_inverse_pred_gain(&a[..d]) > 0, "{a:?}");
    }
}

#[test]
fn inverse_prediction_gain() {
    assert_eq!(lpc_inverse_pred_gain(&[0; 10]), 1 << 30);
    // A single coefficient of 0.5: 1 - 0.25.
    let g = lpc_inverse_pred_gain(&[2048, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert!((g - (3 << 28)).abs() < 1 << 12, "{g}");
    // Unstable, and a DC response of one or more.
    assert_eq!(lpc_inverse_pred_gain(&[4096, 0, 0, 0, 0, 0, 0, 0, 0, 0]), 0);
    assert_eq!(lpc_inverse_pred_gain(&[i16::MAX; 16]), 0);
    assert_eq!(lpc_inverse_pred_gain(&[i16::MIN; 16]), 0);
}

#[test]
fn bandwidth_expansion() {
    let mut a = [4096i16, 4096];
    bwexpander(&mut a, 32768);
    assert_eq!(a, [2048, 1024]);
    let mut b = [1 << 20, 1 << 20];
    bwexpander_32(&mut b, 32768);
    assert_eq!(b, [1 << 19, 1 << 18]);
}
