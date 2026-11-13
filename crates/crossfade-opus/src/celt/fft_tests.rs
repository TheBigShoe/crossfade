use super::*;

#[test]
fn ifft_is_the_inverse_dft() {
    for shift in 0..4 {
        let n = size(shift);
        let fin: Vec<Cpx> = (0..n)
            .map(|k| Cpx {
                r: ((k * 7 % 13) as f32) - 6.0,
                i: ((k * 5 % 11) as f32) - 5.0,
            })
            .collect();
        let mut fout = vec![Cpx::default(); n];
        ifft(shift, &fin, &mut fout);
        for k in (0..n).step_by(7) {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (j, x) in fin.iter().enumerate() {
                let ph = 2.0 * std::f64::consts::PI * (j * k % n) as f64 / n as f64;
                let (s, c) = (libm::sin(ph), libm::cos(ph));
                re += f64::from(x.r) * c - f64::from(x.i) * s;
                im += f64::from(x.r) * s + f64::from(x.i) * c;
            }
            assert!((f64::from(fout[k].r) - re).abs() < 1e-2, "n {n} k {k}");
            assert!((f64::from(fout[k].i) - im).abs() < 1e-2, "n {n} k {k}");
        }
    }
}
