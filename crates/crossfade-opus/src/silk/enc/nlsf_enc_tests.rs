use super::*;
use crate::silk::nlsf::nlsf2a;
use crate::silk::tables_nlsf::{NLSF_CB_NB_MB, NLSF_CB_WB};

/// Some stable whitening filters: resonances spread over the band.
fn spread_nlsfs(d: usize, tilt: i32) -> [i16; MAX_LPC_ORDER] {
    let mut n = [0i16; MAX_LPC_ORDER];
    for (k, v) in n.iter_mut().take(d).enumerate() {
        let base = (k as i32 + 1) * 32768 / (d as i32 + 1);
        *v = (base + tilt * (k as i32 % 3 - 1)) as i16;
    }
    n
}

#[test]
fn a2nlsf_inverts_nlsf2a() {
    for d in [10usize, 16] {
        for tilt in [-600, 0, 400, 900] {
            let want = spread_nlsfs(d, tilt);
            let mut a_q12 = [0i16; MAX_LPC_ORDER];
            nlsf2a(&mut a_q12[..d], &want[..d]);
            let mut a_q16: Vec<i32> = a_q12[..d].iter().map(|&a| i32::from(a) << 4).collect();
            let mut got = [0i16; MAX_LPC_ORDER];
            a2nlsf(&mut got[..d], &mut a_q16);
            for k in 0..d {
                let err = (i32::from(got[k]) - i32::from(want[k])).abs();
                assert!(
                    err < 64,
                    "order {d} tilt {tilt} nlsf {k}: {} vs {}",
                    got[k],
                    want[k]
                );
            }
        }
    }
}

#[test]
fn a2nlsf_of_a_flat_filter_is_evenly_spread() {
    let mut a_q16 = [0i32; 16];
    let mut got = [0i16; 16];
    a2nlsf(&mut got, &mut a_q16);
    for k in 0..16 {
        let want = (k as i32 + 1) * 32768 / 17;
        assert!((i32::from(got[k]) - want).abs() < 64, "{k}: {}", got[k]);
    }
}

#[test]
fn laroia_weights_grow_where_nlsfs_crowd() {
    let mut n = spread_nlsfs(10, 0);
    n[4] = n[5] - 50;
    let mut w = [0i16; 10];
    nlsf_vq_weights_laroia(&mut w, &n[..10]);
    assert!(w[4] > 2 * w[0], "{w:?}");
    assert!(w.iter().all(|&x| x > 0));
}

#[test]
fn the_quantised_nlsfs_are_what_their_indices_decode_to_and_close_to_the_input() {
    for (cb, d) in [(&NLSF_CB_NB_MB, 10usize), (&NLSF_CB_WB, 16)] {
        for tilt in [-600, 0, 400] {
            for signal_type in 0..3 {
                for survivors in [2usize, 8, 16] {
                    let input = spread_nlsfs(d, tilt);
                    let mut q = input;
                    let mut w = [0i16; MAX_LPC_ORDER];
                    nlsf_vq_weights_laroia(&mut w[..d], &input[..d]);
                    let mut idx = [0i8; MAX_LPC_ORDER + 1];
                    let rd =
                        nlsf_encode(&mut idx, &mut q, cb, &w[..d], 3000, survivors, signal_type);
                    assert!(rd > 0);
                    assert!((0..32).contains(&idx[0]));
                    assert!(
                        idx[1..=d].iter().all(|&i| (-10..=10).contains(&i)),
                        "{idx:?}"
                    );
                    let mut dec = [0i16; MAX_LPC_ORDER];
                    nlsf_decode(&mut dec, &idx, cb);
                    assert_eq!(dec, q);
                    for k in 0..d {
                        let err = (i32::from(q[k]) - i32::from(input[k])).abs();
                        assert!(err < 1200, "order {d} nlsf {k}: {} vs {}", q[k], input[k]);
                    }
                }
            }
        }
    }
}

#[test]
fn interpolate_weighs_the_second_vector_by_quarters() {
    let x0 = [0i16, 100, -100];
    let x1 = [400i16, 100, 300];
    let mut xi = [0i16; 3];
    interpolate(&mut xi, &x0, &x1, 1);
    assert_eq!(xi, [100, 100, 0]);
    interpolate(&mut xi, &x0, &x1, 4);
    assert_eq!(xi, x1);
}
