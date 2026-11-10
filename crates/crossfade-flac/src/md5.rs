//! MD5 (RFC 1321). FLAC stores the MD5 of the unencoded samples in STREAMINFO.

const K: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];

/// Index of the message word each step reads.
const G: [usize; 64] = {
    let mut g = [0usize; 64];
    let mut i = 0;
    while i < 64 {
        g[i] = match i / 16 {
            0 => i,
            1 => (5 * i + 1) % 16,
            2 => (3 * i + 5) % 16,
            _ => (7 * i) % 16,
        };
        i += 1;
    }
    g
};

#[cfg(test)]
const S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

#[derive(Debug, Clone)]
pub struct Md5 {
    state: [u32; 4],
    buf: [u8; 64],
    buffered: usize,
    len: u64,
}

impl Default for Md5 {
    fn default() -> Self {
        Self::new()
    }
}

/// The four round functions, one per 16 steps.
#[inline(always)]
fn ff(b: u32, c: u32, d: u32) -> u32 {
    d ^ (b & (c ^ d))
}
#[inline(always)]
fn gg(b: u32, c: u32, d: u32) -> u32 {
    c ^ (d & (b ^ c))
}
#[inline(always)]
fn hh(b: u32, c: u32, d: u32) -> u32 {
    b ^ c ^ d
}
#[inline(always)]
fn ii(b: u32, c: u32, d: u32) -> u32 {
    c ^ (b | !d)
}

/// Fully unrolled, so the message indices, constants and rotations are all immediates and the
/// four state words rotate by renaming instead of moves.
fn compress(state: &mut [u32; 4], block: &[u8; 64]) {
    let mut m = [0u32; 16];
    for (w, c) in m.iter_mut().zip(block.as_chunks::<4>().0) {
        *w = u32::from_le_bytes(*c);
    }
    let [mut a, mut b, mut c, mut d] = *state;
    macro_rules! step {
        ($f:ident, $a:ident, $b:ident, $c:ident, $d:ident, $i:expr, $s:expr) => {
            $a = $b.wrapping_add(
                $a.wrapping_add($f($b, $c, $d))
                    .wrapping_add(m[G[$i]])
                    .wrapping_add(K[$i])
                    .rotate_left($s),
            );
        };
    }
    macro_rules! round {
        ($f:ident, $i:expr, $s:expr) => {
            step!($f, a, b, c, d, $i, $s[0]);
            step!($f, d, a, b, c, $i + 1, $s[1]);
            step!($f, c, d, a, b, $i + 2, $s[2]);
            step!($f, b, c, d, a, $i + 3, $s[3]);
        };
    }
    round!(ff, 0, [7, 12, 17, 22]);
    round!(ff, 4, [7, 12, 17, 22]);
    round!(ff, 8, [7, 12, 17, 22]);
    round!(ff, 12, [7, 12, 17, 22]);
    round!(gg, 16, [5, 9, 14, 20]);
    round!(gg, 20, [5, 9, 14, 20]);
    round!(gg, 24, [5, 9, 14, 20]);
    round!(gg, 28, [5, 9, 14, 20]);
    round!(hh, 32, [4, 11, 16, 23]);
    round!(hh, 36, [4, 11, 16, 23]);
    round!(hh, 40, [4, 11, 16, 23]);
    round!(hh, 44, [4, 11, 16, 23]);
    round!(ii, 48, [6, 10, 15, 21]);
    round!(ii, 52, [6, 10, 15, 21]);
    round!(ii, 56, [6, 10, 15, 21]);
    round!(ii, 60, [6, 10, 15, 21]);
    for (s, v) in state.iter_mut().zip([a, b, c, d]) {
        *s = s.wrapping_add(v);
    }
}

/// The textbook loop the unrolled version must equal.
#[cfg(test)]
fn compress_simple(state: &mut [u32; 4], block: &[u8; 64]) {
    let mut m = [0u32; 16];
    for (w, c) in m.iter_mut().zip(block.as_chunks::<4>().0) {
        *w = u32::from_le_bytes(*c);
    }
    let [mut a, mut b, mut c, mut d] = *state;
    for i in 0..64 {
        let (f, g) = match i / 16 {
            0 => ((b & c) | (!b & d), i),
            1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
            2 => (b ^ c ^ d, (3 * i + 5) % 16),
            _ => (c ^ (b | !d), (7 * i) % 16),
        };
        let f = f.wrapping_add(a).wrapping_add(K[i]).wrapping_add(m[g]);
        a = d;
        d = c;
        c = b;
        b = b.wrapping_add(f.rotate_left(S[i]));
    }
    for (s, v) in state.iter_mut().zip([a, b, c, d]) {
        *s = s.wrapping_add(v);
    }
}

impl Md5 {
    pub fn new() -> Self {
        Self {
            state: [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476],
            buf: [0; 64],
            buffered: 0,
            len: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.len = self.len.wrapping_add(data.len() as u64);
        if self.buffered > 0 {
            let take = (64 - self.buffered).min(data.len());
            self.buf[self.buffered..self.buffered + take].copy_from_slice(&data[..take]);
            self.buffered += take;
            data = &data[take..];
            if self.buffered < 64 {
                return;
            }
            let block = self.buf;
            compress(&mut self.state, &block);
            self.buffered = 0;
        }
        let (blocks, rest) = data.as_chunks::<64>();
        for block in blocks {
            compress(&mut self.state, block);
        }
        self.buf[..rest.len()].copy_from_slice(rest);
        self.buffered = rest.len();
    }

    pub fn finalize(mut self) -> [u8; 16] {
        let bits = self.len.wrapping_mul(8);
        let pad = if self.buffered < 56 {
            56 - self.buffered
        } else {
            120 - self.buffered
        };
        let mut tail = [0u8; 72];
        tail[0] = 0x80;
        tail[pad..pad + 8].copy_from_slice(&bits.to_le_bytes());
        self.update(&tail[..pad + 8]);
        let mut out = [0u8; 16];
        for (o, s) in out.as_chunks_mut::<4>().0.iter_mut().zip(self.state) {
            *o = s.to_le_bytes();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn hex(d: [u8; 16]) -> String {
        d.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn md5(data: &[u8]) -> String {
        let mut m = Md5::new();
        m.update(data);
        hex(m.finalize())
    }

    // RFC 1321 appendix A.5.
    #[test]
    fn rfc_1321_test_suite() {
        let cases: [(&[u8], &str); 7] = [
            (b"", "d41d8cd98f00b204e9800998ecf8427e"),
            (b"a", "0cc175b9c0f1b6a831c399e269772661"),
            (b"abc", "900150983cd24fb0d6963f7d28e17f72"),
            (b"message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            (
                b"abcdefghijklmnopqrstuvwxyz",
                "c3fcd3d76192e4007dfb496cca67e13b",
            ),
            (
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
                "d174ab98d277d9f5a5611c2c9f419d9f",
            ),
            (
                b"12345678901234567890123456789012345678901234567890123456789012345678901234567890",
                "57edf4a22be3c955ac49da2e2107b67a",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(md5(input), want, "{}", String::from_utf8_lossy(input));
        }
    }

    #[test]
    fn padding_boundaries() {
        // 55, 56 and 64 bytes straddle the length-field and block edges.
        assert_eq!(md5(&[b'a'; 55]), "ef1772b6dff9a122358552954ad0df65");
        assert_eq!(md5(&[b'a'; 56]), "3b0c8ac703f828b04c6c197006d17218");
        assert_eq!(md5(&[b'a'; 64]), "014842d480b571495a4a0363793f7367");
    }

    proptest! {
        #[test]
        fn unrolled_compress_equals_the_textbook_loop(
            state in any::<[u32; 4]>(),
            block in prop::collection::vec(any::<u8>(), 64),
        ) {
            let block: [u8; 64] = block.try_into().unwrap();
            let (mut fast, mut slow) = (state, state);
            compress(&mut fast, &block);
            compress_simple(&mut slow, &block);
            prop_assert_eq!(fast, slow);
        }

        #[test]
        fn splitting_the_input_changes_nothing(
            data in prop::collection::vec(any::<u8>(), 0..300),
            cuts in prop::collection::vec(any::<usize>(), 0..6),
        ) {
            let mut points: Vec<usize> = cuts.iter().map(|c| c % (data.len() + 1)).collect();
            points.sort_unstable();
            let mut m = Md5::new();
            let mut at = 0;
            for p in points {
                m.update(&data[at..p]);
                at = p;
            }
            m.update(&data[at..]);
            let mut whole = Md5::new();
            whole.update(&data);
            prop_assert_eq!(m.finalize(), whole.finalize());
        }
    }
}
