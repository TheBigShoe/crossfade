use super::*;

#[test]
fn the_network_gives_libopus_s_probabilities() {
    // libopus 1.5.2's `mlp.c` and `mlp_data.c` (float, gcc -O2 -ffp-contract=off) on the
    // same four feature vectors, one after the other: music, then activity.
    const EXPECTED: [[f32; 2]; 4] = [
        [9.137_901_664e-1, 8.008_917_570e-1],
        [7.970_925_570e-1, 2.222_691_476e-1],
        [8.670_407_534e-1, 9.061_815_143e-1],
        [8.079_605_103e-1, 9.307_498_932e-1],
    ];
    let mut net = Mlp::default();
    for (s, want) in EXPECTED.iter().enumerate() {
        let mut features = [0.0f32; 25];
        for (k, f) in features.iter_mut().enumerate() {
            *f = (((k * 7 + s * 3) % 11) as f32 - 5.0) * 0.25;
        }
        let got = net.run(&features);
        for i in 0..2 {
            assert!(
                (got[i] - want[i]).abs() < 1e-6,
                "step {s}: {got:?}, libopus {want:?}"
            );
        }
    }
}

#[test]
fn reset_forgets_the_recurrent_state() {
    let mut features = [0.0f32; 25];
    features[3] = 1.5;
    let mut a = Mlp::default();
    let first = a.run(&features);
    a.run(&features);
    a.reset();
    assert_eq!(a.run(&features), first);
}
