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

/// The largest band, in coefficients (22 bins of 2.5 ms at 20 ms).
const MAX_N: usize = 176;

/// U(n, k) for n in 0..=MAX_N and k in 0..=MAX_PULSES + 1, built at compile time from
/// `U(n, k) = U(n-1, k) + U(n, k-1) + U(n-1, k-1)`, U(0, 0) = 1, saturating where it exceeds
/// 32 bits: the pulse cache only allows (n, k) whose V(n, k) = U(n, k) + U(n, k+1) fits, and
/// every entry the decoder then reads is no larger. libopus's `CELT_PVQ_U` table, whole.
static U: [[u32; MAX_PULSES + 2]; MAX_N + 1] = {
    let mut u = [[0u32; MAX_PULSES + 2]; MAX_N + 1];
    u[0][0] = 1;
    let mut n = 1;
    while n <= MAX_N {
        let mut k = 1;
        while k < MAX_PULSES + 2 {
            u[n][k] = u[n - 1][k]
                .saturating_add(u[n][k - 1])
                .saturating_add(u[n - 1][k - 1]);
            k += 1;
        }
        n += 1;
    }
    u
};

/// U(n, k), symmetric, as libopus's `CELT_PVQ_U` reads it (k may reach n).
#[inline(always)]
fn u(n: usize, k: usize) -> u32 {
    let (a, b) = if n <= k { (k, n) } else { (n, k) };
    // U(n, k) = U(k, n): the table has every n up to MAX_N for k up to MAX_PULSES + 1.
    if b <= MAX_PULSES + 1 && a <= MAX_N {
        U[a][b]
    } else if a <= MAX_PULSES + 1 && b <= MAX_N {
        U[b][a]
    } else {
        u32::MAX
    }
}

/// `cwrsi` with the table (libopus's current form): the `i`-th vector, O(n + k).
fn cwrsi_table(mut n: usize, mut k: usize, mut i: u32, y: &mut [i32]) {
    let mut at = 0;
    while n > 2 {
        let k0 = k;
        if k >= n {
            let p = u(n, k + 1);
            let s: i32 = -i32::from(i >= p);
            i = i.wrapping_sub(p & s as u32);
            let q = u(n, n);
            let mut p;
            if q > i {
                k = n;
                loop {
                    k -= 1;
                    p = u(k, n);
                    if p <= i || k == 0 {
                        break;
                    }
                }
            } else {
                p = u(n, k);
                while p > i && k > 0 {
                    k -= 1;
                    p = u(n, k);
                }
            }
            i = i.wrapping_sub(p);
            y[at] = ((k0 - k) as i32 + s) ^ s;
        } else {
            let p = u(k, n);
            let q = u(k + 1, n);
            if p <= i && i < q {
                i -= p;
                y[at] = 0;
            } else {
                let s: i32 = -i32::from(i >= q);
                i = i.wrapping_sub(q & s as u32);
                let mut p;
                loop {
                    k = k.saturating_sub(1);
                    p = u(k, n);
                    if p <= i || k == 0 {
                        break;
                    }
                }
                i = i.wrapping_sub(p);
                y[at] = ((k0 - k) as i32 + s) ^ s;
            }
        }
        at += 1;
        n -= 1;
    }
    // n == 2
    let p = 2 * k as u32 + 1;
    let s: i32 = -i32::from(i >= p);
    i = i.wrapping_sub(p & s as u32);
    let k0 = k;
    k = (i.wrapping_add(1) >> 1) as usize;
    if k != 0 {
        i = i.wrapping_sub(2 * k as u32 - 1);
    }
    y[at] = ((k0 as i32 - k as i32) + s) ^ s;
    // n == 1
    let s = -(i as i32);
    y[at + 1] = (k as i32 + s) ^ s;
}

/// `decode_pulses`: reads the index of a vector of `y.len()` (>= 2) integers whose absolute
/// values sum to `k` (1 to [`MAX_PULSES`]) and writes it to `y`.
pub(super) fn decode_pulses(y: &mut [i32], k: usize, ec: &mut Ec<'_, '_>) {
    let n = y.len();
    let mut row = [0u32; MAX_PULSES + 2];
    if n < 2 || k == 0 || k > MAX_PULSES {
        y.fill(0);
        return;
    }
    if n > MAX_N {
        let v = ncwrs_urow(n, k, &mut row);
        let i = ec.uint(v);
        cwrsi(k, i, y, &mut row);
        return;
    }
    let v = u(n, k).wrapping_add(u(n, k + 1));
    let i = ec.uint(v);
    cwrsi_table(n, k, i, y);
}

#[cfg(test)]
#[path = "cwrs_tests.rs"]
pub(super) mod tests;
