//! Decoding constructed streams. Expected output comes from `model`, the specification's
//! decode from spectra on written plainly in f64 (each block's IMDCT by its definition,
//! windowed by its neighbours' sizes on one timeline), or, where the expected spectrum is not
//! worth computing by hand (floor 0, many-post floor 1), from lewton.

use super::*;
use crate::testing::{
    self as w, BitWriter, Book, Floor0, Floor1, Floor1Class, Mapping, Mode, Residue, float32_pack,
};
use std::f64::consts::PI;

// ---------------------------------------------------------------------------------------------
// The model.

/// One decoded block as the model sees it: its size and each channel's spectrum (n/2 lines).
#[derive(Clone)]
struct Block {
    n: usize,
    spectra: Vec<Vec<f64>>,
}

fn slope(s: usize, k: usize) -> f64 {
    let x = libm::sin((k as f64 + 0.5) / s as f64 * PI / 2.0);
    libm::sin(PI / 2.0 * x * x)
}

/// y[t] = sum X[k] cos(2 pi / n (t + 1/2 + n/4)(k + 1/2)), over the lines that are not zero.
fn imdct(x: &[f64]) -> Vec<f64> {
    let n = 2 * x.len();
    let mut y = vec![0.0; n];
    for (k, &v) in x.iter().enumerate() {
        if v == 0.0 {
            continue;
        }
        for (t, yt) in y.iter_mut().enumerate() {
            *yt += v * libm::cos(
                2.0 * PI / n as f64 * (t as f64 + 0.5 + n as f64 / 4.0) * (k as f64 + 0.5),
            );
        }
    }
    y
}

/// Interleaved output from the centre of the first block to the centre of the last.
fn model(blocks: &[Block]) -> Vec<f64> {
    if blocks.len() < 2 {
        return vec![];
    }
    let ch = blocks[0].spectra.len();
    // Room before the first centre for a long block that starts before a short one.
    let origin = blocks.iter().map(|b| b.n).max().unwrap();
    let mut centres = vec![origin];
    for b in 1..blocks.len() {
        centres.push(centres[b - 1] + blocks[b - 1].n / 4 + blocks[b].n / 4);
    }
    let end = blocks
        .iter()
        .zip(&centres)
        .map(|(b, c)| c + b.n / 2)
        .max()
        .unwrap();
    let mut timeline = vec![vec![0.0f64; end]; ch];
    for (b, block) in blocks.iter().enumerate() {
        let n = block.n;
        let start = centres[b] - n / 2;
        let left = blocks.get(b.wrapping_sub(1)).map_or(n, |p| p.n.min(n)) / 2;
        let right = blocks.get(b + 1).map_or(n, |p| p.n.min(n)) / 2;
        for (c, spectrum) in block.spectra.iter().enumerate() {
            let y = imdct(spectrum);
            for (t, &v) in y.iter().enumerate() {
                let weight = if t < n / 2 {
                    let lo = n / 4 - left / 2;
                    if t < lo {
                        0.0
                    } else if t < lo + left {
                        slope(left, t - lo)
                    } else {
                        1.0
                    }
                } else {
                    let hi = 3 * n / 4 - right / 2;
                    if t < hi {
                        1.0
                    } else if t < hi + right {
                        slope(right, right - 1 - (t - hi))
                    } else {
                        0.0
                    }
                };
                timeline[c][start + t] += v * weight;
            }
        }
    }
    let (first, last) = (centres[0], centres[blocks.len() - 1]);
    (first..last)
        .flat_map(|t| timeline.iter().map(move |c| c[t]))
        .collect()
}

fn assert_close(got: &[f32], want: &[f64], what: &str) {
    assert_eq!(got.len(), want.len(), "{what}: length");
    let peak = want.iter().fold(1.0f64, |a, v| a.max(v.abs()));
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        assert!(
            (f64::from(*g) - w).abs() <= 2e-5 * peak,
            "{what}: sample {i}: got {g}, want {w}"
        );
    }
}

/// The specification's floor 1 line (section 9.2.6) for posts (x, y) already multiplied, to n.
#[allow(clippy::needless_range_loop)]
fn floor_line(posts: &[(usize, i32)], n: usize) -> Vec<f64> {
    let mut out = vec![0.0; n];
    let db = |y: i32| f64::from(crate::floor::INVERSE_DB[y as usize]);
    for p in posts.windows(2) {
        let ((x0, y0), (x1, y1)) = (p[0], p[1]);
        let dy = y1 - y0;
        let adx = (x1 - x0) as i32;
        let base = dy / adx;
        let sy = if dy < 0 { base - 1 } else { base + 1 };
        let ady = dy.abs() - base.abs() * adx;
        let (mut y, mut err) = (y0, 0);
        if x0 < n {
            out[x0] = db(y);
        }
        for x in x0 + 1..x1 {
            err += ady;
            if err >= adx {
                err -= adx;
                y += sy;
            } else {
                y += base;
            }
            if x < n {
                out[x] = db(y);
            }
        }
    }
    let &(hx, hy) = posts.last().unwrap();
    for o in out.iter_mut().skip(hx) {
        *o = db(hy);
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Streams.

struct Stream {
    ident: Vec<u8>,
    setup: w::Setup,
    channels: u8,
    block_sizes: [u16; 2],
    packets: Vec<Vec<u8>>,
}

impl Stream {
    fn new(channels: u8, block_sizes: [u16; 2], setup: w::Setup) -> Self {
        Self {
            ident: w::ident(channels, 44_100, block_sizes),
            setup,
            channels,
            block_sizes,
            packets: vec![],
        }
    }

    fn decoder(&self) -> Decoder {
        let ident = Ident::parse(&self.ident).unwrap();
        let setup = Setup::parse(&self.setup.write(self.channels), &ident, usize::MAX).unwrap();
        Decoder::new(&ident, setup).unwrap()
    }

    /// Starts a packet in `mode`, with window flags that agree with the blocks around it.
    fn start(&self, mode: u8, prev: Option<u8>, next: Option<u8>) -> BitWriter {
        let long = |m: Option<u8>| m.is_none_or(|m| self.setup.modes[usize::from(m)].long);
        self.setup.start_packet(mode, long(prev), long(next))
    }

    fn decode_all(&self) -> Vec<f32> {
        let mut d = self.decoder();
        let mut out = vec![];
        for p in &self.packets {
            d.decode(p, &mut out).unwrap();
        }
        out
    }
}

/// lewton's decode of the same packets, concatenated, aligned to ours: where the first block
/// is long and the second short, lewton starts (long - short) / 4 frames later.
fn lewton_pcm(s: &Stream) -> (Vec<f32>, usize) {
    use lewton::audio::{PreviousWindowRight, read_audio_packet_generic};
    use lewton::header::{read_header_ident, read_header_setup};
    use lewton::samples::InterleavedSamples;
    let id = read_header_ident(&s.ident).unwrap();
    let setup = read_header_setup(
        &s.setup.write(s.channels),
        s.channels,
        (id.blocksize_0, id.blocksize_1),
    )
    .unwrap();
    let mut pwr = PreviousWindowRight::new();
    let mut out = vec![];
    for p in &s.packets {
        if let Ok(d) =
            read_audio_packet_generic::<InterleavedSamples<f32>>(&id, &setup, p, &mut pwr)
        {
            out.extend_from_slice(&d.samples);
        }
    }
    let ours = Setup::parse(
        &s.setup.write(s.channels),
        &Ident::parse(&s.ident).unwrap(),
        usize::MAX,
    )
    .unwrap();
    let sizes: Vec<usize> = s
        .packets
        .iter()
        .filter_map(|p| ours.modes().block_size(p).ok())
        .take(2)
        .collect();
    let skip = match sizes[..] {
        [a, b] if a > b => (a - b) / 4,
        _ => 0,
    };
    (out, skip * usize::from(s.channels))
}

/// lewton is the oracle only for blocks of 256 and more: below that its IMDCT (stb_vorbis's
/// algorithm) is wrong, which these tests found; the model covers the small sizes.
fn assert_matches_lewton(s: &Stream, what: &str) {
    if s.block_sizes[0] < 256 {
        return;
    }
    let ours = s.decode_all();
    let (theirs, skip) = lewton_pcm(s);
    let ours = &ours[skip.min(ours.len())..];
    let n = ours.len().min(theirs.len());
    assert!(n > 0, "{what}: nothing to compare");
    let peak = ours.iter().fold(1.0f32, |a, v| a.max(v.abs()));
    for i in 0..n {
        // A clamped sample says nothing about the value under it.
        if ours[i].abs() >= OUTPUT_LIMIT {
            continue;
        }
        assert!(
            (ours[i] - theirs[i]).abs() <= 1e-5 * peak,
            "{what}: sample {i}: ours {} lewton {}",
            ours[i],
            theirs[i]
        );
    }
}

/// Integer values from -2 to 2 for a book of 2^bits entries of `dims` dimensions, entry e's
/// values being its base-5 digits less 2.
fn values_book(bits: u8, dims: u16) -> Book {
    let entries = 1u32 << bits;
    let mut v = vec![];
    for e in 0..entries {
        let mut x = e;
        for _ in 0..dims {
            v.push(x % 5);
            x /= 5;
        }
    }
    Book {
        dims,
        ..Book::flat(bits)
    }
    .with_values(-2, &v)
}

fn values_of(book: &Book, e: u32) -> Vec<f64> {
    let mut x = e;
    (0..book.dims)
        .map(|_| {
            let v = f64::from(x % 5) - 2.0;
            x /= 5;
            v
        })
        .collect()
}

/// A tiny deterministic generator.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u32
    }
    fn below(&mut self, n: u32) -> u32 {
        self.next() % n
    }
}

/// Writes floor 1 with only its two implicit posts (simple_setup's floor): a straight line.
fn put_line_floor(wr: &mut BitWriter, y0: u32, y1: u32) {
    wr.put_bit(true);
    wr.put(u64::from(y0), 8);
    wr.put(u64::from(y1), 8);
}

/// The model spectrum of simple_setup's floor for a line from y0 to y1.
fn line_curve(y0: u32, y1: u32, half: usize) -> Vec<f64> {
    floor_line(&[(0, y0 as i32), (256, y1 as i32)], half)
}

/// Residue data for simple_setup (one classification, the class book's single codeword one
/// bit): for each partition, a class bit per coded vector, then that partition of each
/// vector. Returns the entries written per vector.
fn put_residue(
    wr: &mut BitWriter,
    book: &Book,
    vectors: usize,
    partitions: usize,
    per_partition: usize,
    rng: &mut Rng,
) -> Vec<Vec<u32>> {
    let words = book.codewords();
    let entries = book.lengths.len() as u32;
    let mut written = vec![vec![]; vectors];
    for _ in 0..partitions {
        for _ in 0..vectors {
            wr.put_bit(false);
        }
        for v in written.iter_mut() {
            for _ in 0..per_partition {
                let e = rng.below(entries);
                Book::encode_with(&words, wr, e);
                v.push(e);
            }
        }
    }
    written
}

/// Spreads residue entries into a vector the way residue `kind` lays them out.
fn lay_out(
    kind: u16,
    book: &Book,
    entries: &[u32],
    begin: usize,
    psize: usize,
    len: usize,
) -> Vec<f64> {
    let dims = usize::from(book.dims);
    let per = psize / dims;
    let mut v = vec![0.0; len];
    for (p, chunk) in entries.chunks(per).enumerate() {
        let base = begin + p * psize;
        for (i, &e) in chunk.iter().enumerate() {
            for (k, x) in values_of(book, e).into_iter().enumerate() {
                let at = if kind == 0 {
                    base + i + k * per
                } else {
                    base + i * dims + k
                };
                v[at] += x;
            }
        }
    }
    v
}

fn times(a: &[f64], b: &[f64]) -> Vec<f64> {
    a.iter().zip(b).map(|(x, y)| x * y).collect()
}

fn modes_of(seq: &str) -> Vec<u8> {
    seq.bytes().map(|b| u8::from(b == b'L')).collect()
}

// ---------------------------------------------------------------------------------------------
// Tests.

#[test]
fn the_first_packet_yields_nothing_and_each_next_from_centre_to_centre() {
    let book = values_book(2, 1);
    let mut s = Stream::new(1, [256, 2048], w::simple_setup(1, book, 0, 1));
    let seq = modes_of("SLLSSLSS");
    for (i, &m) in seq.iter().enumerate() {
        let mut wr = s.start(m, i.checked_sub(1).map(|j| seq[j]), seq.get(i + 1).copied());
        put_line_floor(&mut wr, 100, 100);
        s.packets.push(wr.into_bytes());
    }
    let mut d = s.decoder();
    let mut out = vec![];
    let counts: Vec<usize> = s
        .packets
        .iter()
        .map(|p| d.decode(p, &mut out).unwrap())
        .collect();
    assert_eq!(counts, [0, 576, 1024, 576, 128, 576, 576, 128]);
    assert_eq!(out.len(), counts.iter().sum::<usize>());
    assert!(out.iter().all(|&v| v == 0.0));
}

#[test]
fn a_spectrum_through_floor_1_and_the_imdct_matches_the_specification() {
    let book = values_book(2, 1);
    for (sizes, seq) in [
        ([64u16, 256u16], "SSLLSLS"),
        ([256, 2048], "LLSSSLLS"),
        ([256, 256], "SLSL"),
        ([2048, 2048], "SSS"),
    ] {
        let half = usize::from(sizes[1]) / 2;
        let mut s = Stream::new(1, sizes, w::simple_setup(1, book.clone(), half as u32, 8));
        let seq = modes_of(seq);
        let mut rng = Rng(u64::from(sizes[0]));
        let mut blocks = vec![];
        for (i, &m) in seq.iter().enumerate() {
            let n = usize::from(sizes[usize::from(m)]);
            let mut wr = s.start(m, i.checked_sub(1).map(|j| seq[j]), seq.get(i + 1).copied());
            // Floors below 200 keep the sum of a thousand random lines well under the limit.
            let (y0, y1) = (rng.below(200), rng.below(200));
            put_line_floor(&mut wr, y0, y1);
            let entries = put_residue(&mut wr, &book, 1, n / 2 / 8, 8, &mut rng);
            s.packets.push(wr.into_bytes());
            let residue = lay_out(1, &book, &entries[0], 0, 8, n / 2);
            blocks.push(Block {
                n,
                spectra: vec![times(&residue, &line_curve(y0, y1, n / 2))],
            });
        }
        assert_close(&s.decode_all(), &model(&blocks), &format!("{sizes:?}"));
        assert_matches_lewton(&s, &format!("{sizes:?}"));
    }
}

#[test]
fn every_block_size_from_64_to_8192_decodes() {
    // A sparse spectrum keeps the model's IMDCT cheap at 8192.
    for e0 in 6..=13u32 {
        for e1 in [e0, 13] {
            let sizes = [1u16 << e0, 1u16 << e1];
            let book = values_book(1, 1);
            let mut setup = w::simple_setup(1, book.clone(), 64, 64);
            setup.residues[0].begin = 0;
            let mut s = Stream::new(1, sizes, setup);
            let seq = modes_of("SLLSL");
            let mut blocks = vec![];
            let mut rng = Rng(u64::from(e0 * 31 + e1));
            for (i, &m) in seq.iter().enumerate() {
                let n = usize::from(sizes[usize::from(m)]);
                let mut wr = s.start(m, i.checked_sub(1).map(|j| seq[j]), seq.get(i + 1).copied());
                put_line_floor(&mut wr, 200, 180);
                let coded = 64.min(n / 2);
                let parts = coded / 64;
                let entries = put_residue(&mut wr, &book, 1, parts, 64, &mut rng);
                s.packets.push(wr.into_bytes());
                let residue = lay_out(1, &book, &entries[0], 0, 64, n / 2);
                blocks.push(Block {
                    n,
                    spectra: vec![times(&residue, &line_curve(200, 180, n / 2))],
                });
            }
            assert_close(&s.decode_all(), &model(&blocks), &format!("{sizes:?}"));
        }
    }
}

#[test]
fn residue_types_0_1_and_2_lay_out_their_vectors_as_specified() {
    let book = values_book(3, 2);
    for kind in 0..3u16 {
        let (begin, end, psize) = (8usize, 40usize, 8usize);
        let mut setup = w::simple_setup(kind, book.clone(), end as u32, psize as u32);
        setup.residues[0].begin = begin as u32;
        let mut s = Stream::new(2, [256, 512], setup);
        let seq = modes_of("SSLS");
        let mut rng = Rng(9 + u64::from(kind));
        let mut blocks = vec![];
        for (i, &m) in seq.iter().enumerate() {
            let n = usize::from(s.block_sizes[usize::from(m)]);
            let half = n / 2;
            let mut wr = s.start(m, i.checked_sub(1).map(|j| seq[j]), seq.get(i + 1).copied());
            put_line_floor(&mut wr, 150, 150);
            put_line_floor(&mut wr, 140, 160);
            let spectra = if kind == 2 {
                // One interleaved vector of 2 * half; limits apply to it.
                let (b, e) = (begin.min(2 * half), end.min(2 * half));
                let parts = (e - b) / psize;
                let entries = put_residue(&mut wr, &book, 1, parts, psize / 2, &mut rng);
                let big = lay_out(1, &book, &entries[0], b, psize, 2 * half);
                vec![
                    big.iter().step_by(2).copied().collect::<Vec<_>>(),
                    big.iter().skip(1).step_by(2).copied().collect(),
                ]
            } else {
                let (b, e) = (begin.min(half), end.min(half));
                let parts = (e - b) / psize;
                let entries = put_residue(&mut wr, &book, 2, parts, psize / 2, &mut rng);
                entries
                    .iter()
                    .map(|en| lay_out(kind, &book, en, b, psize, half))
                    .collect()
            };
            s.packets.push(wr.into_bytes());
            blocks.push(Block {
                n,
                spectra: vec![
                    times(&spectra[0], &line_curve(150, 150, half)),
                    times(&spectra[1], &line_curve(140, 160, half)),
                ],
            });
        }
        assert_close(&s.decode_all(), &model(&blocks), &format!("type {kind}"));
        assert_matches_lewton(&s, &format!("type {kind}"));
    }
}

/// The inverse of square polar mapping (section 4.3.5), for the test's own use.
fn uncouple(m: f64, a: f64) -> (f64, f64) {
    match (m > 0.0, a > 0.0) {
        (true, true) => (m, m - a),
        (true, false) => (m + a, m),
        (false, true) => (m, m + a),
        (false, false) => (m - a, m),
    }
}

#[test]
fn coupled_channels_are_restored_from_magnitude_and_angle() {
    assert_eq!(uncouple(2.0, 1.0), (2.0, 1.0));
    assert_eq!(uncouple(2.0, -1.0), (1.0, 2.0));
    assert_eq!(uncouple(-2.0, 1.0), (-2.0, -1.0));
    assert_eq!(uncouple(-2.0, -1.0), (-1.0, -2.0));
    let book = values_book(4, 2);
    let mut setup = w::simple_setup(2, book.clone(), 128, 16);
    setup.mappings[0].coupling = vec![(0, 1)];
    let mut s = Stream::new(2, [256, 512], setup);
    let seq = modes_of("LSSL");
    let mut rng = Rng(77);
    let mut blocks = vec![];
    for (i, &m) in seq.iter().enumerate() {
        let n = usize::from(s.block_sizes[usize::from(m)]);
        let half = n / 2;
        let mut wr = s.start(m, i.checked_sub(1).map(|j| seq[j]), seq.get(i + 1).copied());
        put_line_floor(&mut wr, 220, 220);
        put_line_floor(&mut wr, 230, 210);
        let parts = 128.min(2 * half) / 16;
        let entries = put_residue(&mut wr, &book, 1, parts, 8, &mut rng);
        s.packets.push(wr.into_bytes());
        let big = lay_out(1, &book, &entries[0], 0, 16, 2 * half);
        let (mut l, mut r) = (vec![0.0; half], vec![0.0; half]);
        for k in 0..half {
            (l[k], r[k]) = uncouple(big[2 * k], big[2 * k + 1]);
        }
        blocks.push(Block {
            n,
            spectra: vec![
                times(&l, &line_curve(220, 220, half)),
                times(&r, &line_curve(230, 210, half)),
            ],
        });
    }
    assert_close(&s.decode_all(), &model(&blocks), "coupled");
    assert_matches_lewton(&s, "coupled");
}

#[test]
fn an_unused_floor_silences_its_channel_and_skips_its_residue_unless_coupled() {
    let book = values_book(3, 2);
    for coupled in [false, true] {
        let mut setup = w::simple_setup(1, book.clone(), 64, 8);
        if coupled {
            setup.mappings[0].coupling = vec![(0, 1)];
        }
        let mut s = Stream::new(2, [256, 256], setup);
        let mut rng = Rng(5);
        let mut blocks = vec![];
        for _ in 0..4 {
            let mut wr = s.start(0, None, None);
            put_line_floor(&mut wr, 190, 190);
            wr.put_bit(false);
            // Coupling makes channel 1's residue coded too; without it only channel 0's is.
            let vectors = if coupled { 2 } else { 1 };
            let entries = put_residue(&mut wr, &book, vectors, 8, 4, &mut rng);
            s.packets.push(wr.into_bytes());
            let a = lay_out(1, &book, &entries[0], 0, 8, 128);
            let left = if coupled {
                let b = lay_out(1, &book, &entries[1], 0, 8, 128);
                a.iter()
                    .zip(&b)
                    .map(|(&m, &an)| uncouple(m, an).0)
                    .collect()
            } else {
                a
            };
            blocks.push(Block {
                n: 256,
                spectra: vec![times(&left, &line_curve(190, 190, 128)), vec![0.0; 128]],
            });
        }
        let out = s.decode_all();
        assert!(out.iter().skip(1).step_by(2).all(|&v| v == 0.0));
        assert_close(&out, &model(&blocks), &format!("coupled {coupled}"));
        assert_matches_lewton(&s, &format!("coupled {coupled}"));
    }
}

#[test]
fn submaps_route_each_channel_to_its_floor_and_residue() {
    let book0 = values_book(2, 1);
    let book1 = values_book(3, 2);
    let mut setup = w::simple_setup(1, book0.clone(), 32, 4);
    setup.books.push(book1.clone());
    setup.residues.push(Residue {
        kind: 0,
        begin: 0,
        end: 32,
        partition_size: 8,
        classbook: 1,
        books: vec![[Some(2), None, None, None, None, None, None, None]],
    });
    setup.floors.push(setup.floors[0].clone());
    setup.mappings[0] = Mapping {
        submaps: vec![(0, 0), (1, 1)],
        coupling: vec![],
        mux: vec![1, 0, 1],
    };
    let mut s = Stream::new(3, [256, 256], setup);
    let mut rng = Rng(11);
    let mut blocks = vec![];
    for _ in 0..3 {
        let mut wr = s.start(0, None, None);
        for y in [100, 120, 140] {
            put_line_floor(&mut wr, y, y);
        }
        // Submap 0 (channel 1), then submap 1 (channels 0 and 2).
        let e1 = put_residue(&mut wr, &book0, 1, 8, 4, &mut rng);
        let e02 = put_residue(&mut wr, &book1, 2, 4, 4, &mut rng);
        s.packets.push(wr.into_bytes());
        blocks.push(Block {
            n: 256,
            spectra: vec![
                times(
                    &lay_out(0, &book1, &e02[0], 0, 8, 128),
                    &line_curve(100, 100, 128),
                ),
                times(
                    &lay_out(1, &book0, &e1[0], 0, 4, 128),
                    &line_curve(120, 120, 128),
                ),
                times(
                    &lay_out(0, &book1, &e02[1], 0, 8, 128),
                    &line_curve(140, 140, 128),
                ),
            ],
        });
    }
    assert_close(&s.decode_all(), &model(&blocks), "submaps");
    assert_matches_lewton(&s, "submaps");
}

#[test]
fn several_passes_and_classifications_add_up() {
    // Two classifications (class book of two one-bit entries), class 0 coding passes 0 and 2,
    // class 1 only pass 1; three different books.
    let books = [values_book(2, 1), values_book(3, 1), values_book(1, 1)];
    let mut setup = w::simple_setup(1, books[0].clone(), 32, 8);
    setup.books[1] = Book::flat(1);
    setup.books.push(books[1].clone());
    setup.books.push(books[2].clone());
    setup.residues[0].books = vec![
        [Some(0), None, Some(3), None, None, None, None, None],
        [None, Some(2), None, None, None, None, None, None],
    ];
    let mut s = Stream::new(1, [256, 256], setup);
    let mut rng = Rng(3);
    let mut blocks = vec![];
    let words: Vec<_> = books.iter().map(Book::codewords).collect();
    for _ in 0..4 {
        let mut wr = s.start(0, None, None);
        put_line_floor(&mut wr, 170, 170);
        let classes: Vec<u32> = (0..4).map(|_| rng.below(2)).collect();
        let mut v = vec![0.0; 128];
        for pass in 0..3 {
            for (p, &c) in classes.iter().enumerate() {
                if pass == 0 {
                    wr.put_bit(c == 1);
                }
                let book = match (c, pass) {
                    (0, 0) => 0,
                    (0, 2) => 2,
                    (1, 1) => 1,
                    _ => continue,
                };
                for i in 0..8 {
                    let e = rng.below(books[book].lengths.len() as u32);
                    Book::encode_with(&words[book], &mut wr, e);
                    v[p * 8 + i] += values_of(&books[book], e)[0];
                }
            }
        }
        s.packets.push(wr.into_bytes());
        blocks.push(Block {
            n: 256,
            spectra: vec![times(&v, &line_curve(170, 170, 128))],
        });
    }
    assert_close(&s.decode_all(), &model(&blocks), "passes");
    assert_matches_lewton(&s, "passes");
}

/// A floor 1 with three classes (one with a masterbook and subclasses, one with an unused
/// subclass) and posts on both sides of the block's end.
fn rich_floor1() -> Floor1 {
    Floor1 {
        partition_classes: vec![0, 1, 2, 1],
        classes: vec![
            Floor1Class {
                dims: 2,
                subclass_bits: 0,
                masterbook: 0,
                subbooks: vec![2],
            },
            Floor1Class {
                dims: 3,
                subclass_bits: 1,
                masterbook: 3,
                subbooks: vec![-1, 2],
            },
            Floor1Class {
                dims: 1,
                subclass_bits: 2,
                masterbook: 4,
                subbooks: vec![2, 5, -1, 5],
            },
        ],
        multiplier: 2,
        range_bits: 7,
        xs: vec![60, 20, 3, 9, 40, 100, 33, 70, 90],
    }
}

#[test]
fn floor_1_with_classes_subclasses_and_many_posts_matches_lewton() {
    let book = values_book(3, 1);
    for multiplier in 1..=4u8 {
        let mut setup = w::simple_setup(1, book.clone(), 64, 8);
        let mut f = rich_floor1();
        f.multiplier = multiplier;
        let range = f.range();
        setup.floors[0] = w::Floor::One(f.clone());
        setup.books.push(Book::flat(6)); // 2: Y values
        setup.books.push(Book::flat(1)); // 3: masterbook, cval 0..1
        setup.books.push(Book::flat(3)); // 4: masterbook, cval 0..7
        setup.books.push(Book::flat(5)); // 5: Y values
        let mut s = Stream::new(1, [256, 256], setup.clone());
        let mut rng = Rng(u64::from(multiplier));
        for _ in 0..6 {
            let mut wr = s.start(0, None, None);
            let cvals: Vec<u32> = vec![0, rng.below(2), rng.below(8), rng.below(2)];
            let mut ys = vec![rng.below(range), rng.below(range)];
            let mut at = 0;
            for (p, &class) in f.partition_classes.iter().enumerate() {
                let c = &f.classes[usize::from(class)];
                let mut cval = cvals[p];
                for _ in 0..c.dims {
                    let b = c.subbooks[(cval & ((1 << c.subclass_bits) - 1)) as usize];
                    cval >>= c.subclass_bits;
                    let most = if b < 0 {
                        1
                    } else {
                        setup.books[b as usize].lengths.len() as u32
                    };
                    ys.push(rng.below(most.min(range)));
                    at += 1;
                }
            }
            assert_eq!(at, f.xs.len());
            f.encode(&mut wr, &setup.books, &ys, &cvals);
            put_residue(&mut wr, &book, 1, 8, 8, &mut rng);
            s.packets.push(wr.into_bytes());
        }
        assert_matches_lewton(&s, &format!("multiplier {multiplier}"));
        assert!(s.decode_all().iter().any(|&v| v != 0.0));
    }
}

#[test]
fn floor_0_matches_lewton() {
    let book = values_book(3, 1);
    // The LSP book: entry e is (0.25 + 0.125 (e % 4), 0.75 + 0.125 (e / 4)), so that the
    // coefficients rise and stay below pi, as an encoder's do.
    let mut v = vec![];
    for e in 0..16u32 {
        v.extend([e % 4, 4 + e / 4]);
    }
    let mut lsp = Book {
        dims: 2,
        ..Book::flat(4)
    }
    .with_values(0, &v);
    if let w::Lookup::Values { minimum, delta, .. } = &mut lsp.lookup {
        *minimum = float32_pack(0.25);
        *delta = float32_pack(0.125);
    }
    // lewton panics on order 1 ((order - 3) / 2 underflows) and on 32 or more amplitude bits
    // (an i32 shift), so neither is compared here; `floor_0_of_order_1_and_wide_amplitudes`
    // checks them on their own.
    for (order, amp_bits) in [(4u8, 6u8), (5, 8), (3, 6), (2, 20)] {
        let mut setup = w::simple_setup(1, book.clone(), 64, 8);
        setup.books.push(lsp.clone());
        setup.floors[0] = w::Floor::Zero(Floor0 {
            order,
            rate: 22_050,
            bark_map_size: 128,
            amplitude_bits: amp_bits,
            amplitude_offset: 30,
            books: vec![2, 2],
        });
        let mut s = Stream::new(1, [256, 512], setup);
        let seq = modes_of("SLSLL");
        let mut rng = Rng(u64::from(order));
        let words = lsp.codewords();
        for (i, &m) in seq.iter().enumerate() {
            let mut wr = s.start(m, i.checked_sub(1).map(|j| seq[j]), seq.get(i + 1).copied());
            // Small amplitudes keep the curve's peaks, near the LSP roots, in range.
            let amp = 1 + u64::from(rng.below(4));
            wr.put(amp, u32::from(amp_bits));
            wr.put(u64::from(rng.below(2)), 2);
            for _ in 0..usize::from(order).div_ceil(2) {
                Book::encode_with(&words, &mut wr, rng.below(16));
            }
            put_residue(&mut wr, &book, 1, 8, 8, &mut rng);
            s.packets.push(wr.into_bytes());
        }
        let out = s.decode_all();
        let within = out
            .iter()
            .filter(|v| v.abs() < OUTPUT_LIMIT && **v != 0.0)
            .count();
        assert!(
            within > out.len() / 2,
            "order {order}: {within} of {}",
            out.len()
        );
        assert_matches_lewton(&s, &format!("order {order}"));
    }
}

#[test]
fn floor_0_of_order_1_and_wide_amplitudes() {
    // Order 1 has no p terms and one q term: p = 1 - w^2, q = (cos c0 - w)^2. With amplitude
    // a of b bits and offset o, each line is exp(0.11512925 (a o / (2^b - 1) / sqrt(p + q) - o)).
    let book = values_book(1, 1);
    let lsp = Book {
        dims: 1,
        ..Book::flat(1)
    }
    .with_values(1, &[0, 1]);
    for amp_bits in [6u8, 63] {
        let mut setup = w::simple_setup(1, book.clone(), 0, 1);
        setup.books.push(lsp.clone());
        setup.floors[0] = w::Floor::Zero(Floor0 {
            order: 1,
            rate: 8000,
            bark_map_size: 64,
            amplitude_bits: amp_bits,
            amplitude_offset: 10,
            books: vec![2],
        });
        let ident = Ident::parse(&w::ident(1, 8000, [256, 256])).unwrap();
        let parsed = Setup::parse(&setup.write(1), &ident, usize::MAX).unwrap();
        let Floor::Zero(f) = &parsed.floors[0] else {
            unreachable!()
        };
        let mut wr = BitWriter::new();
        let amp = (1u64 << (amp_bits - 1)) + 3;
        wr.put(amp, u32::from(amp_bits));
        wr.put(0, 1); // book number: ilog(1) bits
        Book::encode_with(&lsp.codewords(), &mut wr, 1); // c0 = 2
        let bytes = wr.into_bytes();
        let mut coeffs = [0.0f32; 256];
        let got_amp = f.decode(&parsed.codebooks, &mut BitReader::new(&bytes), &mut coeffs);
        assert_eq!(got_amp, Some(amp));
        assert_eq!(coeffs[0], 2.0);
        let mut spectrum = vec![1.0f32; 128];
        f.apply(false, amp, &coeffs, &mut spectrum, &mut [0.0; 256]);
        for (i, &v) in spectrum.iter().enumerate() {
            let w = f64::from(f.cos_omega(false)[i]);
            let (p, q) = (1.0 - w * w, (libm::cos(2.0) - w).powi(2));
            let common = amp as f64 * 10.0 / ((1u128 << amp_bits) - 1) as f64;
            let want = libm::exp(0.115_129_25 * (common / (p + q).sqrt() - 10.0));
            assert!(
                (f64::from(v) - want).abs() <= 1e-5 * want,
                "{amp_bits} bits, line {i}: {v} vs {want}"
            );
        }
    }
}

#[test]
fn a_zero_floor_0_amplitude_or_a_missing_book_leaves_the_channel_silent() {
    let book = values_book(3, 1);
    let mut setup = w::simple_setup(1, book.clone(), 64, 8);
    setup.books.push(values_book(2, 2));
    setup.floors[0] = w::Floor::Zero(Floor0 {
        order: 2,
        rate: 8000,
        bark_map_size: 64,
        amplitude_bits: 4,
        amplitude_offset: 40,
        books: vec![2, 2, 2],
    });
    let mut s = Stream::new(1, [64, 64], setup);
    let mut rng = Rng(1);
    for (amp, booknum) in [(0u64, 0u64), (3, 3), (0, 0)] {
        let mut wr = s.start(0, None, None);
        wr.put(amp, 4);
        if amp > 0 {
            wr.put(booknum, 2);
        }
        put_residue(&mut wr, &book, 1, 8, 8, &mut rng);
        s.packets.push(wr.into_bytes());
    }
    let out = s.decode_all();
    assert_eq!(out.len(), 64);
    assert!(out.iter().all(|&v| v == 0.0));
}

#[test]
fn a_packet_cut_short_ends_its_spectrum_early() {
    let book = values_book(2, 1);
    for kind in [0u16, 1] {
        let mut s = Stream::new(1, [64, 64], w::simple_setup(kind, book.clone(), 32, 8));
        let mut rng = Rng(21);
        let mut blocks = vec![];
        // A cut after `c` whole entries of the third partition: the floor and the packet head
        // take 19 bits and each partition 17 (a class bit and eight 2-bit codewords), so
        // c = 1 and c = 5 end on a byte boundary, where a packet ends.
        for cut in [None, Some(1usize), Some(5), None, Some(1)] {
            let mut wr = s.start(0, None, None);
            put_line_floor(&mut wr, 180, 180);
            assert_eq!(wr.bits(), 19);
            let entries = put_residue(&mut wr, &book, 1, 4, 8, &mut rng);
            let mut bytes = wr.into_bytes();
            let mut entries = entries[0].clone();
            if let Some(c) = cut {
                let bits = 19 + 2 * 17 + 1 + 2 * c;
                assert_eq!(bits % 8, 0);
                bytes.truncate(bits / 8);
                // Type 1 adds what it read; type 0 needs the whole partition (libvorbis).
                entries.truncate(16 + if kind == 1 { c } else { 0 });
            }
            s.packets.push(bytes);
            let residue = lay_out(kind, &book, &entries, 0, 8, 32);
            blocks.push(Block {
                n: 64,
                spectra: vec![times(&residue, &line_curve(180, 180, 32))],
            });
        }
        assert_close(&s.decode_all(), &model(&blocks), &format!("type {kind}"));
    }
}

#[test]
fn a_floor_cut_short_silences_the_channel() {
    let book = values_book(2, 1);
    let mut s = Stream::new(1, [64, 64], w::simple_setup(1, book, 32, 8));
    for _ in 0..3 {
        let mut wr = s.start(0, None, None);
        wr.put_bit(true);
        wr.put(200, 8);
        s.packets.push(wr.into_bytes()); // y1 missing
    }
    let out = s.decode_all();
    assert_eq!(out.len(), 64);
    assert!(out.iter().all(|&v| v == 0.0));
}

#[test]
fn a_bad_packet_is_an_error_for_that_packet_only() {
    let book = values_book(2, 1);
    let mut setup = w::simple_setup(1, book.clone(), 32, 8);
    setup.modes.push(Mode {
        long: false,
        mapping: 0,
    });
    let mut s = Stream::new(1, [64, 128], setup);
    let mut rng = Rng(8);
    for _ in 0..5 {
        let mut wr = s.start(0, None, None);
        put_line_floor(&mut wr, 160, 170);
        put_residue(&mut wr, &book, 1, 4, 8, &mut rng);
        s.packets.push(wr.into_bytes());
    }
    let good = s.decode_all();
    let mut d = s.decoder();
    let mut out = vec![];
    // Mode 3 does not exist (two mode bits: 0b11), a header, an empty packet.
    let bad: [&[u8]; 4] = [&[0b110], &[0x01, b'v'], &[], &[0b1110]];
    for (i, p) in s.packets.iter().enumerate() {
        let before = out.len();
        let e = d.decode(bad[i % 4], &mut out);
        assert!(matches!(e, Err(Error::BadPacket(_))), "{e:?}");
        assert_eq!(out.len(), before);
        d.decode(p, &mut out).unwrap();
    }
    assert_eq!(out, good);
}

#[test]
fn reset_forgets_the_overlap() {
    let book = values_book(2, 1);
    let mut s = Stream::new(2, [64, 256], w::simple_setup(2, book.clone(), 64, 8));
    let mut rng = Rng(4);
    let seq = modes_of("SLLS");
    for (i, &m) in seq.iter().enumerate() {
        let mut wr = s.start(m, i.checked_sub(1).map(|j| seq[j]), seq.get(i + 1).copied());
        put_line_floor(&mut wr, 160, 170);
        put_line_floor(&mut wr, 150, 170);
        put_residue(&mut wr, &book, 1, 8, 8, &mut rng);
        s.packets.push(wr.into_bytes());
    }
    let mut fresh = s.decoder();
    let mut want = vec![];
    for p in &s.packets[2..] {
        fresh.decode(p, &mut want).unwrap();
    }
    let mut d = s.decoder();
    let mut out = vec![];
    d.decode(&s.packets[0], &mut out).unwrap();
    d.decode(&s.packets[1], &mut out).unwrap();
    d.reset();
    out.clear();
    assert_eq!(d.decode(&s.packets[2], &mut out).unwrap(), 0);
    d.decode(&s.packets[3], &mut out).unwrap();
    assert_eq!(out, want);
}

#[test]
fn the_same_bits_however_the_output_is_gathered() {
    let book = values_book(3, 2);
    let mut s = Stream::new(2, [64, 512], w::simple_setup(2, book.clone(), 128, 16));
    let mut rng = Rng(6);
    let seq = modes_of("SLSSLLS");
    for (i, &m) in seq.iter().enumerate() {
        let mut wr = s.start(m, i.checked_sub(1).map(|j| seq[j]), seq.get(i + 1).copied());
        put_line_floor(&mut wr, 200, 100);
        put_line_floor(&mut wr, 190, 120);
        let n = usize::from(s.block_sizes[usize::from(m)]);
        put_residue(&mut wr, &book, 1, 128.min(n) / 16, 8, &mut rng);
        s.packets.push(wr.into_bytes());
    }
    let all = s.decode_all();
    let mut d = s.decoder();
    let mut pieces = vec![];
    for p in &s.packets {
        let mut out = vec![1.0; 3];
        let n = d.decode(p, &mut out).unwrap();
        assert_eq!(out.len(), 3 + 2 * n);
        pieces.extend_from_slice(&out[3..]);
    }
    assert_eq!(
        all.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        pieces.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
    );
}

#[test]
fn output_stays_finite_and_bounded_whatever_the_values() {
    let mut book = Book::flat(1);
    book.lookup = w::Lookup::Values {
        kind: 1,
        minimum: 0x7FE0_0001, // infinity in f32
        delta: float32_pack(1.0),
        value_bits: 1,
        sequence: false,
        multiplicands: vec![0, 1],
    };
    let mut s = Stream::new(1, [64, 64], w::simple_setup(1, book.clone(), 32, 8));
    let mut rng = Rng(2);
    for _ in 0..4 {
        let mut wr = s.start(0, None, None);
        put_line_floor(&mut wr, 255, 255);
        put_residue(&mut wr, &book, 1, 4, 8, &mut rng);
        s.packets.push(wr.into_bytes());
    }
    let out = s.decode_all();
    assert_eq!(out.len(), 96);
    assert!(out.iter().all(|v| v.is_finite() && v.abs() <= OUTPUT_LIMIT));
}

#[test]
fn channels_come_out_in_the_order_asked_for() {
    let book = values_book(2, 1);
    let mut s = Stream::new(3, [64, 64], w::simple_setup(1, book.clone(), 32, 8));
    let mut rng = Rng(12);
    for _ in 0..3 {
        let mut wr = s.start(0, None, None);
        for y in [100, 150, 200] {
            put_line_floor(&mut wr, y, y);
        }
        put_residue(&mut wr, &book, 3, 4, 8, &mut rng);
        s.packets.push(wr.into_bytes());
    }
    let plain = s.decode_all();
    let mut d = s.decoder();
    assert!(!d.set_channel_order(&[0, 0, 1]));
    assert!(!d.set_channel_order(&[0, 1]));
    assert!(d.set_channel_order(&[2, 0, 1]));
    let mut out = vec![];
    for p in &s.packets {
        d.decode(p, &mut out).unwrap();
    }
    for (a, b) in plain.chunks(3).zip(out.chunks(3)) {
        assert_eq!([b[2], b[0], b[1]], [a[0], a[1], a[2]]);
    }
}

#[test]
fn the_decoder_says_how_much_it_holds() {
    let book = values_book(2, 1);
    let s = Stream::new(6, [256, 8192], w::simple_setup(1, book, 32, 8));
    let ident = Ident::parse(&s.ident).unwrap();
    let setup = Setup::parse(&s.setup.write(6), &ident, usize::MAX).unwrap();
    let need = Decoder::bytes_needed(&setup);
    assert!(need >= 6 * 8192 * 4, "{need}");
    assert!(need < 2 << 20, "{need}");
    let other = Ident::parse(&w::ident(2, 44_100, [256, 8192])).unwrap();
    assert!(matches!(
        Decoder::new(&other, setup),
        Err(Error::Malformed(_))
    ));
}
