//! The PVQ codebook index (`celt/cwrs.c`): the `i`-th vector of `n` integers whose absolute
//! values sum to `k`, through the U(N, K) recurrence (the reference's generic, small-footprint
//! path, exact for every size the pulse cache allows: V(N, K) fits in 32 bits there).

use super::ec::Ec;

/// The largest pulse count a band codes (`MAX_PULSES` of `rate.h`, `get_pulses(40)`).
pub(super) const MAX_PULSES: usize = 128;

/// `unext`: the next row of `u[i][j] = u[i-1][j] + u[i][j-1] + u[i-1][j-1]` with base `ui0`.
fn unext(ui: &mut [u32], mut ui0: u32) {
    for j in 1..ui.len() {
        let ui1 = ui[j].wrapping_add(ui[j - 1]).wrapping_add(ui0);
        ui[j - 1] = ui0;
        ui0 = ui1;
    }
    if let Some(last) = ui.last_mut() {
        *last = ui0;
    }
}

/// `uprev`: the previous row, inverting [`unext`].
fn uprev(ui: &mut [u32], mut ui0: u32) {
    for j in 1..ui.len() {
        let ui1 = ui[j].wrapping_sub(ui[j - 1]).wrapping_sub(ui0);
        ui[j - 1] = ui0;
        ui0 = ui1;
    }
    if let Some(last) = ui.last_mut() {
        *last = ui0;
    }
}

/// `ncwrs_urow`: fills `u[0..=k + 1]` with U(n, 0..=k + 1) and returns V(n, k) (n >= 2,
/// k >= 1).
fn ncwrs_urow(n: usize, k: usize, u: &mut [u32]) -> u32 {
    let len = k + 2;
    u[0] = 0;
    u[1] = 1;
    for (j, v) in u.iter_mut().enumerate().take(len).skip(2) {
        *v = ((j as u32) << 1) - 1;
    }
    for _ in 2..n {
        unext(&mut u[1..len], 1);
    }
    u[k].wrapping_add(u[k + 1])
}

/// `cwrsi`: the `i`-th combination, from row `n` of U in `u` (destroyed).
fn cwrsi(mut k: usize, mut i: u32, y: &mut [i32], u: &mut [u32]) {
    for yj_out in y.iter_mut() {
        let p = u[k + 1];
        let s: i32 = -i32::from(i >= p);
        i = i.wrapping_sub(p & s as u32);
        let yj = k as i32;
        let mut p = u[k];
        while p > i && k > 0 {
            k -= 1;
            p = u[k];
        }
        i = i.wrapping_sub(p);
        let yj = yj - k as i32;
        *yj_out = (yj + s) ^ s;
        uprev(&mut u[..k + 2], 0);
    }
}

/// `decode_pulses`: reads the index of a vector of `y.len()` (>= 2) integers whose absolute
/// values sum to `k` (1 to [`MAX_PULSES`]) and writes it to `y`.
pub(super) fn decode_pulses(y: &mut [i32], k: usize, ec: &mut Ec<'_, '_>) {
    let n = y.len();
    let mut u = [0u32; MAX_PULSES + 2];
    if n < 2 || k == 0 || k > MAX_PULSES {
        y.fill(0);
        return;
    }
    let v = ncwrs_urow(n, k, &mut u);
    let i = ec.uint(v);
    cwrsi(k, i, y, &mut u);
}

#[cfg(test)]
#[path = "cwrs_tests.rs"]
pub(super) mod tests;
