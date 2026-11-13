use super::*;
use crate::celt::test_util::encode;
use crate::range::RangeDecoder;
use crate::testing::range_encoder::Op;

/// `icwrs`: the index of `y` and V(n, k), the encoder's side.
pub(in crate::celt) fn icwrs(y: &[i32]) -> (u32, u32) {
    let n = y.len();
    let k: usize = y.iter().map(|v| v.unsigned_abs() as usize).sum();
    let mut u = vec![0u32; k + 2];
    for (j, v) in u.iter_mut().enumerate().skip(1) {
        *v = ((j as u32) << 1) - 1;
    }
    let mut kk = y[n - 1].unsigned_abs() as usize;
    let mut i = u32::from(y[n - 1] < 0);
    let j = n - 2;
    i = i.wrapping_add(u[kk]);
    kk += y[j].unsigned_abs() as usize;
    if y[j] < 0 {
        i = i.wrapping_add(u[kk + 1]);
    }
    for j in (0..n - 2).rev() {
        unext(&mut u, 0);
        i = i.wrapping_add(u[kk]);
        kk += y[j].unsigned_abs() as usize;
        if y[j] < 0 {
            i = i.wrapping_add(u[kk + 1]);
        }
    }
    (i, u[kk].wrapping_add(u[kk + 1]))
}

/// V(n, k) in 128 bits, to know which sizes fit in 32.
fn v128(n: usize, k: usize) -> u128 {
    let mut row = vec![1u128; k + 1];
    row[1..].fill(2);
    // V(1, k) = 2 for k > 0; V(n, k) = V(n-1, k) + V(n, k-1) + V(n-1, k-1).
    for _ in 1..n {
        let mut next = vec![1u128; k + 1];
        for kk in 1..=k {
            next[kk] = row[kk] + next[kk - 1] + row[kk - 1];
        }
        row = next;
    }
    row[k]
}

fn round_trip(y: &[i32]) {
    let k: usize = y.iter().map(|v| v.unsigned_abs() as usize).sum();
    let (i, v) = icwrs(y);
    assert!(i < v);
    let frame = encode(&[Op::Uint { val: i, ft: v }]);
    let mut dec = RangeDecoder::new(&frame);
    let mut ec = Ec::new(&mut dec);
    let mut got = vec![0; y.len()];
    decode_pulses(&mut got, k, &mut ec);
    assert_eq!(got, y);
}

#[test]
fn v_is_the_number_of_vectors() {
    // V(2, 2) = 8: (+-2, 0), (0, +-2), (+-1, +-1).
    let mut u = [0; 8];
    assert_eq!(ncwrs_urow(2, 2, &mut u), 8);
    for n in 2..30 {
        for k in 1..20 {
            let want = v128(n, k);
            if want < 1 << 32 {
                assert_eq!(
                    u128::from(ncwrs_urow(n, k, &mut [0; 32])),
                    want,
                    "V({n}, {k})"
                );
            }
        }
    }
}

#[test]
fn every_small_vector_round_trips() {
    // Every vector of n <= 4, k <= 4, and every index of V(n, k) maps back.
    for n in 2..=4 {
        for k in 1..=4 {
            let mut u = [0u32; 8];
            let v = ncwrs_urow(n, k, &mut u);
            let mut seen = std::collections::BTreeSet::new();
            for i in 0..v {
                let mut u = [0u32; 8];
                ncwrs_urow(n, k, &mut u);
                let mut y = vec![0; n];
                cwrsi(k, i, &mut y, &mut u);
                assert_eq!(
                    y.iter().map(|v| v.unsigned_abs() as usize).sum::<usize>(),
                    k
                );
                assert_eq!(icwrs(&y).0, i);
                assert!(seen.insert(y));
            }
        }
    }
}

#[test]
fn the_largest_sizes_the_cache_allows_round_trip() {
    // For each band size the decoder can meet unsplit, the largest K whose V fits in 32
    // bits, with pulses spread to both ends and both signs.
    for n in [
        2usize, 3, 4, 6, 8, 12, 16, 18, 24, 32, 36, 48, 64, 72, 96, 144, 176,
    ] {
        let mut k = 1;
        while k < MAX_PULSES && v128(n, k + 1) < 1 << 32 {
            k += 1;
        }
        let mut y = vec![0; n];
        y[0] = -(k as i32 / 2);
        y[n - 1] = (k - k / 2) as i32;
        round_trip(&y);
        let mut y = vec![0; n];
        for p in 0..k {
            let at = (p * 7) % n;
            y[at] += if p % 3 == 0 { -1 } else { 1 };
        }
        if y.iter()
            .map(|v: &i32| v.unsigned_abs() as usize)
            .sum::<usize>()
            == k
        {
            round_trip(&y);
        }
    }
}
