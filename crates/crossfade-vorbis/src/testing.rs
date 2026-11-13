//! A Vorbis bitstream writer for tests: a bit packer and builders for the three headers and the
//! pieces of audio packets, with every field a raw number so that a test can write invalid
//! streams as easily as valid ones, and [`ogg_file`] to put a stream in Ogg pages. It chooses
//! nothing for you and is not an encoder. Hidden from the documentation: for this crate's tests
//! and its users' (the engine's reader and command-line tests).

/// Packs bits least significant first, as Vorbis reads them.
#[derive(Debug, Clone, Default)]
pub struct BitWriter {
    bytes: Vec<u8>,
    bits: usize,
}

impl BitWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// The low `n` bits of `value` (`n` at most 64).
    pub fn put(&mut self, value: u64, n: u32) {
        for i in 0..n {
            if self.bits.is_multiple_of(8) {
                self.bytes.push(0);
            }
            if (value >> i) & 1 == 1 {
                *self.bytes.last_mut().expect("pushed above") |= 1 << (self.bits % 8);
            }
            self.bits += 1;
        }
    }

    pub fn put_bit(&mut self, bit: bool) {
        self.put(u64::from(bit), 1);
    }

    pub fn put_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.put(u64::from(b), 8);
        }
    }

    /// A codeword of `len` bits given most significant bit first, the order Vorbis reads it.
    pub fn put_codeword(&mut self, code: u32, len: u8) {
        for i in (0..u32::from(len)).rev() {
            self.put_bit((code >> i) & 1 == 1);
        }
    }

    pub fn bits(&self) -> usize {
        self.bits
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// The identification header.
pub fn ident(channels: u8, sample_rate: u32, block_sizes: [u16; 2]) -> Vec<u8> {
    let mut b = vec![1];
    b.extend_from_slice(b"vorbis");
    b.extend_from_slice(&0u32.to_le_bytes());
    b.push(channels);
    b.extend_from_slice(&sample_rate.to_le_bytes());
    b.extend_from_slice(&[0; 12]);
    let exp = |n: u16| n.trailing_zeros() as u8;
    b.push(exp(block_sizes[0]) | (exp(block_sizes[1]) << 4));
    b.push(1);
    b
}

/// The comment header.
pub fn comment(vendor: &str, entries: &[&str]) -> Vec<u8> {
    let mut b = vec![3];
    b.extend_from_slice(b"vorbis");
    b.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    b.extend_from_slice(vendor.as_bytes());
    b.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for e in entries {
        b.extend_from_slice(&(e.len() as u32).to_le_bytes());
        b.extend_from_slice(e.as_bytes());
    }
    b.push(1);
    b
}

/// Vorbis's float format for a value with at most 21 significant bits (exact; panics otherwise).
pub fn float32_pack(v: f64) -> u32 {
    if v == 0.0 {
        return 0;
    }
    let sign = if v < 0.0 { 0x8000_0000 } else { 0 };
    let mut m = v.abs();
    let mut e: i32 = 0;
    while m >= f64::from(1u32 << 21) {
        m /= 2.0;
        e += 1;
    }
    while m < f64::from(1u32 << 20) {
        m *= 2.0;
        e -= 1;
    }
    assert!(m.fract() == 0.0, "{v} needs more than 21 bits");
    let exp = (e + 788) as u32;
    assert!(exp < 1024, "{v} out of range");
    sign | (exp << 21) | m as u32
}

/// A codebook's value lookup.
#[derive(Debug, Clone, PartialEq)]
pub enum Lookup {
    None,
    /// Type 1 (a lattice, `multiplicands` holding `lookup1_values` values) or type 2 (one value
    /// per scalar of every entry).
    Values {
        kind: u8,
        /// Packed with [`float32_pack`].
        minimum: u32,
        delta: u32,
        value_bits: u8,
        sequence: bool,
        multiplicands: Vec<u32>,
    },
}

/// A codebook; a length of 0 marks an unused entry (written sparse).
#[derive(Debug, Clone, PartialEq)]
pub struct Book {
    pub dims: u16,
    pub lengths: Vec<u8>,
    /// Written in the ordered (run-length) form; the lengths must then be non-decreasing.
    pub ordered: bool,
    pub lookup: Lookup,
}

impl Book {
    /// A scalar book (one dimension, no lookup).
    pub fn scalar(lengths: Vec<u8>) -> Self {
        Self {
            dims: 1,
            lengths,
            ordered: false,
            lookup: Lookup::None,
        }
    }

    /// A book of `2^bits` entries all `bits` long: entry `e` is `e`'s bits, most significant
    /// first.
    pub fn flat(bits: u8) -> Self {
        Self::scalar(vec![bits; 1 << bits])
    }

    /// A type 2 lookup giving each entry the `dims` values listed for it (integers from
    /// `minimum` in steps of 1).
    pub fn with_values(mut self, minimum: i32, values: &[u32]) -> Self {
        let most = values.iter().copied().max().unwrap_or(0);
        self.lookup = Lookup::Values {
            kind: 2,
            minimum: float32_pack(f64::from(minimum)),
            delta: float32_pack(1.0),
            value_bits: (32 - most.leading_zeros()).max(1) as u8,
            sequence: false,
            multiplicands: values.to_vec(),
        };
        self
    }

    /// The codeword of every entry, assigned as the specification says (the lowest codeword
    /// of the entry's length that neither prefixes nor extends one already given, in entry
    /// order): each codeword is a block of the interval [0, 2^32), and this finds the lowest
    /// free aligned block. Deliberately not the decoder's algorithm. `None` for an unused
    /// entry or one that no longer fits (an overspecified tree).
    pub fn codewords(&self) -> Vec<Option<(u32, u8)>> {
        use std::collections::BTreeMap;
        // Taken space as disjoint, non-adjacent [start, end) intervals, by start.
        let mut taken: BTreeMap<u64, u64> = BTreeMap::new();
        self.lengths
            .iter()
            .map(|&len| {
                if len == 0 {
                    return None;
                }
                let size = 1u64 << (32 - u32::from(len));
                let mut at = 0u64;
                let start = loop {
                    let cand = at.div_ceil(size) * size;
                    if cand + size > 1 << 32 {
                        return None;
                    }
                    match taken.range(..cand + size).next_back() {
                        Some((_, &end)) if end > cand => at = end,
                        _ => break cand,
                    }
                };
                let mut lo = start;
                let mut hi = start + size;
                if let Some((&s, &e)) = taken.range(..lo).next_back()
                    && e == lo
                {
                    taken.remove(&s);
                    lo = s;
                }
                if let Some(e) = taken.remove(&hi) {
                    hi = e;
                }
                taken.insert(lo, hi);
                Some(((start / size) as u32, len))
            })
            .collect()
    }

    /// Writes `entry` with codewords from [`codewords`](Self::codewords); panics if it has none.
    pub fn encode_with(words: &[Option<(u32, u8)>], w: &mut BitWriter, entry: u32) {
        let (code, len) = words[entry as usize].expect("entry has a codeword");
        w.put_codeword(code, len);
    }

    /// Writes the codeword of `entry`; panics if it has none.
    pub fn encode(&self, w: &mut BitWriter, entry: u32) {
        let (code, len) = self.codewords()[entry as usize].expect("entry has a codeword");
        w.put_codeword(code, len);
    }

    pub fn write(&self, w: &mut BitWriter) {
        w.put(0x56_4342, 24);
        w.put(u64::from(self.dims), 16);
        let entries = self.lengths.len() as u32;
        w.put(u64::from(entries), 24);
        w.put_bit(self.ordered);
        if self.ordered {
            let first = self.lengths.first().copied().unwrap_or(1);
            w.put(u64::from(first - 1), 5);
            let mut at = 0u32;
            let mut len = first;
            while at < entries {
                let run = self.lengths[at as usize..]
                    .iter()
                    .take_while(|&&l| l == len)
                    .count() as u32;
                w.put(u64::from(run), ilog(entries - at));
                at += run;
                len += 1;
            }
        } else {
            let sparse = self.lengths.contains(&0);
            w.put_bit(sparse);
            for &l in &self.lengths {
                if sparse {
                    w.put_bit(l != 0);
                    if l == 0 {
                        continue;
                    }
                }
                w.put(u64::from(l) - 1, 5);
            }
        }
        match &self.lookup {
            Lookup::None => w.put(0, 4),
            Lookup::Values {
                kind,
                minimum,
                delta,
                value_bits,
                sequence,
                multiplicands,
            } => {
                w.put(u64::from(*kind), 4);
                w.put(u64::from(*minimum), 32);
                w.put(u64::from(*delta), 32);
                w.put(u64::from(*value_bits) - 1, 4);
                w.put_bit(*sequence);
                for &m in multiplicands {
                    w.put(u64::from(m), u32::from(*value_bits));
                }
            }
        }
    }
}

pub fn ilog(v: u32) -> u32 {
    32 - v.leading_zeros()
}

/// Floor type 0 (LSP).
#[derive(Debug, Clone, PartialEq)]
pub struct Floor0 {
    pub order: u8,
    pub rate: u16,
    pub bark_map_size: u16,
    pub amplitude_bits: u8,
    pub amplitude_offset: u8,
    pub books: Vec<u8>,
}

/// One class of a type 1 floor.
#[derive(Debug, Clone, PartialEq)]
pub struct Floor1Class {
    pub dims: u8,
    pub subclass_bits: u8,
    pub masterbook: u8,
    /// `1 << subclass_bits` books; -1 for none.
    pub subbooks: Vec<i16>,
}

/// Floor type 1 (piecewise linear).
#[derive(Debug, Clone, PartialEq)]
pub struct Floor1 {
    pub partition_classes: Vec<u8>,
    pub classes: Vec<Floor1Class>,
    pub multiplier: u8,
    pub range_bits: u8,
    /// The X list after the implicit 0 and `1 << range_bits`.
    pub xs: Vec<u32>,
}

impl Floor1 {
    pub fn range(&self) -> u32 {
        [256, 128, 86, 64][usize::from(self.multiplier - 1)]
    }

    /// Writes one channel's floor: `ys` holds the raw (still predicted) value of every post,
    /// the first two included, and `cvals` the master codebook value of each partition (0
    /// where a class has no subclasses).
    pub fn encode(&self, w: &mut BitWriter, books: &[Book], ys: &[u32], cvals: &[u32]) {
        w.put_bit(true);
        let bits = ilog(self.range() - 1);
        w.put(u64::from(ys[0]), bits);
        w.put(u64::from(ys[1]), bits);
        let mut at = 2;
        for (p, &class) in self.partition_classes.iter().enumerate() {
            let c = &self.classes[usize::from(class)];
            let mut cval = cvals[p];
            if c.subclass_bits > 0 {
                books[usize::from(c.masterbook)].encode(w, cval);
            }
            let csub = (1u32 << c.subclass_bits) - 1;
            for _ in 0..c.dims {
                let book = c.subbooks[(cval & csub) as usize];
                cval >>= c.subclass_bits;
                if book >= 0 {
                    books[book as usize].encode(w, ys[at]);
                } else {
                    assert_eq!(ys[at], 0, "post {at} has no book");
                }
                at += 1;
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Floor {
    Zero(Floor0),
    One(Floor1),
}

impl Floor {
    pub fn write(&self, w: &mut BitWriter) {
        match self {
            Floor::Zero(f) => {
                w.put(0, 16);
                w.put(u64::from(f.order), 8);
                w.put(u64::from(f.rate), 16);
                w.put(u64::from(f.bark_map_size), 16);
                w.put(u64::from(f.amplitude_bits), 6);
                w.put(u64::from(f.amplitude_offset), 8);
                w.put(f.books.len() as u64 - 1, 4);
                for &b in &f.books {
                    w.put(u64::from(b), 8);
                }
            }
            Floor::One(f) => {
                w.put(1, 16);
                w.put(f.partition_classes.len() as u64, 5);
                for &c in &f.partition_classes {
                    w.put(u64::from(c), 4);
                }
                for c in &f.classes {
                    w.put(u64::from(c.dims) - 1, 3);
                    w.put(u64::from(c.subclass_bits), 2);
                    if c.subclass_bits > 0 {
                        w.put(u64::from(c.masterbook), 8);
                    }
                    for &b in &c.subbooks {
                        w.put((i64::from(b) + 1) as u64, 8);
                    }
                }
                w.put(u64::from(f.multiplier) - 1, 2);
                w.put(u64::from(f.range_bits), 4);
                for &x in &f.xs {
                    w.put(u64::from(x), u32::from(f.range_bits));
                }
            }
        }
    }
}

/// A residue of type 0, 1 or 2.
#[derive(Debug, Clone, PartialEq)]
pub struct Residue {
    pub kind: u16,
    pub begin: u32,
    pub end: u32,
    pub partition_size: u32,
    pub classbook: u8,
    /// One row per classification: the book of each of the eight passes, if any.
    pub books: Vec<[Option<u8>; 8]>,
}

impl Residue {
    pub fn write(&self, w: &mut BitWriter) {
        w.put(u64::from(self.kind), 16);
        w.put(u64::from(self.begin), 24);
        w.put(u64::from(self.end), 24);
        w.put(u64::from(self.partition_size) - 1, 24);
        w.put(self.books.len() as u64 - 1, 6);
        w.put(u64::from(self.classbook), 8);
        for row in &self.books {
            let mask = row
                .iter()
                .enumerate()
                .fold(0u64, |m, (i, b)| m | (u64::from(b.is_some()) << i));
            w.put(mask & 7, 3);
            w.put_bit(mask > 7);
            if mask > 7 {
                w.put(mask >> 3, 5);
            }
        }
        for row in &self.books {
            for b in row.iter().flatten() {
                w.put(u64::from(*b), 8);
            }
        }
    }
}

/// A type 0 mapping.
#[derive(Debug, Clone, PartialEq)]
pub struct Mapping {
    /// (floor, residue) of each submap.
    pub submaps: Vec<(u8, u8)>,
    /// (magnitude, angle) channel pairs.
    pub coupling: Vec<(u8, u8)>,
    /// The submap of each channel; written only with more than one submap.
    pub mux: Vec<u8>,
}

impl Mapping {
    pub fn write(&self, w: &mut BitWriter, channels: u8) {
        w.put(0, 16);
        let many = self.submaps.len() > 1;
        w.put_bit(many);
        if many {
            w.put(self.submaps.len() as u64 - 1, 4);
        }
        w.put_bit(!self.coupling.is_empty());
        if !self.coupling.is_empty() {
            w.put(self.coupling.len() as u64 - 1, 8);
            let bits = ilog(u32::from(channels) - 1);
            for &(m, a) in &self.coupling {
                w.put(u64::from(m), bits);
                w.put(u64::from(a), bits);
            }
        }
        w.put(0, 2);
        if many {
            for &m in &self.mux {
                w.put(u64::from(m), 4);
            }
        }
        for &(f, r) in &self.submaps {
            w.put(0, 8);
            w.put(u64::from(f), 8);
            w.put(u64::from(r), 8);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mode {
    pub long: bool,
    pub mapping: u8,
}

/// The setup header.
#[derive(Debug, Clone, PartialEq)]
pub struct Setup {
    pub books: Vec<Book>,
    pub floors: Vec<Floor>,
    pub residues: Vec<Residue>,
    pub mappings: Vec<Mapping>,
    pub modes: Vec<Mode>,
}

impl Setup {
    pub fn write(&self, channels: u8) -> Vec<u8> {
        let mut w = BitWriter::new();
        w.put(5, 8);
        w.put_bytes(b"vorbis");
        w.put(self.books.len() as u64 - 1, 8);
        for b in &self.books {
            b.write(&mut w);
        }
        w.put(0, 6);
        w.put(0, 16);
        w.put(self.floors.len() as u64 - 1, 6);
        for f in &self.floors {
            f.write(&mut w);
        }
        w.put(self.residues.len() as u64 - 1, 6);
        for r in &self.residues {
            r.write(&mut w);
        }
        w.put(self.mappings.len() as u64 - 1, 6);
        for m in &self.mappings {
            m.write(&mut w, channels);
        }
        w.put(self.modes.len() as u64 - 1, 6);
        for m in &self.modes {
            w.put_bit(m.long);
            w.put(0, 16);
            w.put(0, 16);
            w.put(u64::from(m.mapping), 8);
        }
        w.put_bit(true);
        w.into_bytes()
    }

    /// Starts an audio packet in `mode`: the packet type bit, the mode number and, for a long
    /// block, the previous and next window flags.
    pub fn start_packet(&self, mode: u8, prev_long: bool, next_long: bool) -> BitWriter {
        let mut w = BitWriter::new();
        w.put_bit(false);
        w.put(u64::from(mode), ilog(self.modes.len() as u32 - 1));
        if self.modes[usize::from(mode)].long {
            w.put_bit(prev_long);
            w.put_bit(next_long);
        }
        w
    }
}

/// A one-floor, one-residue, one-mapping setup for `channels` channels with two modes (short
/// and long): floor 1 with posts at 0 and `x_end` only (a straight line between the two values
/// written per packet), residue `kind` over the whole spectrum with `book` (a type 2 lookup)
/// on pass 0 for its one classification. Books: 0 is the residue's (given), 1 the class book
/// (one entry, so its single codeword costs a bit).
pub fn simple_setup(kind: u16, residue_book: Book, end: u32, partition_size: u32) -> Setup {
    Setup {
        books: vec![residue_book, Book::scalar(vec![1])],
        floors: vec![Floor::One(Floor1 {
            partition_classes: vec![],
            classes: vec![],
            multiplier: 1,
            range_bits: 8,
            xs: vec![],
        })],
        residues: vec![Residue {
            kind,
            begin: 0,
            end,
            partition_size,
            classbook: 1,
            books: vec![[Some(0), None, None, None, None, None, None, None]],
        }],
        mappings: vec![Mapping {
            submaps: vec![(0, 0)],
            coupling: vec![],
            mux: vec![],
        }],
        modes: vec![
            Mode {
                long: false,
                mapping: 0,
            },
            Mode {
                long: true,
                mapping: 0,
            },
        ],
    }
}

/// How [`ogg_file`] lays a stream out in Ogg pages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OggLayout {
    pub serial: u32,
    /// Audio packets per page, cycled.
    pub packets_per_page: Vec<usize>,
    /// The granule position of the first decoded frame: negative for an encoder's priming (the
    /// first page's granule is that much less than its packets yield), positive for a stream
    /// cut out of a longer one.
    pub start: i64,
    /// Frames the last page's granule leaves off the end (the encoder's padding).
    pub end_trim: i64,
}

impl Default for OggLayout {
    fn default() -> Self {
        Self {
            serial: 0x5652_4253,
            packets_per_page: vec![4],
            start: 0,
            end_trim: 0,
        }
    }
}

/// The sample frames each audio packet yields when decoded in order from the start, as the
/// decoder counts them: none for the first, none for one it cannot decode (which leaves the
/// overlap alone), the previous block's quarter plus this one's otherwise.
pub fn yields(ident: &[u8], setup: &[u8], audio: &[Vec<u8>]) -> Vec<i64> {
    let id = crate::Ident::parse(ident).expect("a valid identification header");
    let s = crate::Setup::parse(setup, &id, usize::MAX).expect("a valid setup header");
    let mut prev = None;
    audio
        .iter()
        .map(|p| match s.modes().block_size(p) {
            Ok(n) => {
                let y = prev.map_or(0, |pb| crate::packet_frames(pb, n));
                prev = Some(n);
                y as i64
            }
            Err(_) => 0,
        })
        .collect()
}

/// An Ogg Vorbis file: the identification header alone on the first page, the comment and
/// setup headers on the second, then the audio packets with granule positions from [`yields`]
/// and `layout`. Every page carries its CRC.
pub fn ogg_file(headers: [&[u8]; 3], audio: &[Vec<u8>], layout: &OggLayout) -> Vec<u8> {
    ogg_file_with_yields(
        headers,
        audio,
        &yields(headers[0], headers[2], audio),
        layout,
    )
}

/// As [`ogg_file`], with the granules counting `ys` frames for each packet (an encoder that
/// counts differently from the decoder).
pub fn ogg_file_with_yields(
    headers: [&[u8]; 3],
    audio: &[Vec<u8>],
    ys: &[i64],
    layout: &OggLayout,
) -> Vec<u8> {
    let mut out = Vec::new();
    let mut sequence = 0u32;
    let mut page = |packets: &[&[u8]], granule: i64, flags: u8, out: &mut Vec<u8>| {
        let mut lacing = Vec::new();
        let mut body = Vec::new();
        for p in packets {
            lacing.extend(std::iter::repeat_n(255u8, p.len() / 255));
            lacing.push((p.len() % 255) as u8);
            body.extend_from_slice(p);
        }
        assert!(lacing.len() <= 255, "too many segments for one page");
        let start = out.len();
        out.extend_from_slice(b"OggS\0");
        out.push(flags);
        out.extend_from_slice(&granule.to_le_bytes());
        out.extend_from_slice(&layout.serial.to_le_bytes());
        out.extend_from_slice(&sequence.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.push(lacing.len() as u8);
        out.extend_from_slice(&lacing);
        out.extend_from_slice(&body);
        let crc = ogg_crc(&out[start..]);
        out[start + 22..start + 26].copy_from_slice(&crc.to_le_bytes());
        sequence += 1;
    };
    page(&[headers[0]], 0, 0x02, &mut out);
    let last_flags = if audio.is_empty() { 0x04 } else { 0 };
    page(&[headers[1], headers[2]], 0, last_flags, &mut out);
    let mut pos = layout.start;
    let mut at = 0;
    let mut k = 0;
    while at < audio.len() {
        let n =
            layout.packets_per_page[k % layout.packets_per_page.len()].clamp(1, audio.len() - at);
        k += 1;
        pos += ys[at..at + n].iter().sum::<i64>();
        let last = at + n == audio.len();
        let granule = if last { pos - layout.end_trim } else { pos };
        let refs: Vec<&[u8]> = audio[at..at + n].iter().map(Vec::as_slice).collect();
        page(&refs, granule, if last { 0x04 } else { 0 }, &mut out);
        at += n;
    }
    out
}

/// Ogg's CRC-32 (polynomial 0x04C11DB7, no reflection, no final complement).
pub fn ogg_crc(data: &[u8]) -> u32 {
    let mut crc = 0u32;
    for &b in data {
        crc ^= u32::from(b) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// A small valid stream for readers' and front ends' tests: see [`sample_stream`].
#[derive(Debug, Clone)]
pub struct SampleStream {
    /// Identification, comment (vendor "crossfade test", TITLE=Tone, ARTIST=Nobody) and setup.
    pub headers: [Vec<u8>; 3],
    pub audio: Vec<Vec<u8>>,
    pub setup: Setup,
}

/// `channels` channels at 32 kHz, block sizes 256 and 2048, one audio packet per letter of
/// `seq` (S short, L long, X a packet whose mode does not exist), each with a residue type 2
/// spectrum drawn from `seed` and a quiet flat floor per channel.
pub fn sample_stream(channels: u8, seq: &str, seed: u64) -> SampleStream {
    let book = Book {
        dims: 2,
        ..Book::flat(3)
    }
    .with_values(-2, &(0..16).map(|v| v % 5).collect::<Vec<_>>());
    let mut setup = simple_setup(2, book.clone(), 512, 16);
    // A third mode, so that mode 3 does not exist (two mode bits).
    setup.modes.push(Mode {
        long: false,
        mapping: 0,
    });
    let words = book.codewords();
    let mut x = seed;
    let modes: Vec<u8> = seq
        .bytes()
        .filter(|&b| b != b'X')
        .map(|b| u8::from(b == b'L'))
        .collect();
    let mut audio = vec![];
    let mut k = 0;
    for b in seq.bytes() {
        if b == b'X' {
            audio.push(vec![0b110]);
            continue;
        }
        let m = modes[k];
        let prev = k.checked_sub(1).is_none_or(|j| modes[j] == 1);
        let next = modes.get(k + 1).is_none_or(|&n| n == 1);
        k += 1;
        let mut wr = setup.start_packet(m, prev, next);
        for c in 0..channels {
            wr.put_bit(true);
            wr.put(170 + 10 * u64::from(c % 8), 8);
            wr.put(150, 8);
        }
        let half = if m == 1 { 1024 } else { 128 };
        for _ in 0..(512.min(half * usize::from(channels)) / 16) {
            wr.put_bit(false);
            for _ in 0..8 {
                x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                Book::encode_with(&words, &mut wr, (x >> 61) as u32);
            }
        }
        audio.push(wr.into_bytes());
    }
    SampleStream {
        headers: [
            ident(channels, 32_000, [256, 2048]),
            comment("crossfade test", &["TITLE=Tone", "ARTIST=Nobody"]),
            setup.write(channels),
        ],
        audio,
        setup,
    }
}
