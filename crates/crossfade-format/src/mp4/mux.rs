//! The writer: `ftyp`, `mdat` with the audio as packets arrive, then `moov` once the sample
//! tables are known. The header of `mdat` is patched at the end, so the sink must seek.
//!
//! Media time is the stream's own time base (the sample rate), which is also the movie's
//! timescale: every length is then a whole number of frames, and the reader's rounding never
//! has anything to round.

use super::build::{boxed, full, too_big};
use super::{ilst, sample_entry};
use crossfade_core::{Error, MediaSink, Muxer, Packet, StreamInfo, Tags};
use std::io::SeekFrom;

/// `ftyp`: an iTunes audio file, with the brands such files carry.
const FTYP: &[u8; 28] = b"\0\0\0\x1cftypM4A \0\0\0\0M4A mp42isom";
/// Where the `mdat` header goes, and how long the file's head is: `ftyp`, then 16 bytes that
/// are an 8-byte `free` and a 32-bit `mdat` header, or one 64-bit `mdat` header when the data
/// needs it (see [`mdat_header`]).
const MDAT_AT: u64 = FTYP.len() as u64;
const DATA_AT: u64 = MDAT_AT + 16;

struct Mp4Muxer {
    sink: Box<dyn MediaSink>,
    stream: StreamInfo,
    ilst: Vec<u8>,
    /// The codec's header, checked against the stream; the encoder may give it late.
    cookie: Option<Vec<u8>>,
    /// Where the next byte goes.
    pos: u64,
    sizes: Vec<u32>,
    /// `stts` as (samples, duration) runs.
    durations: Vec<(u32, u32)>,
    /// The first packet's pts, and where the next one must start, in frames.
    start: Option<i64>,
    next_pts: i64,
}

/// What the movie plays out of the media: `priming` frames are skipped, `audible` are played.
#[derive(Debug, Clone, Copy)]
struct Timeline {
    total: u64,
    priming: u64,
    audible: u64,
}

/// An MP4 writer for `stream`.
pub fn create(
    mut sink: Box<dyn MediaSink>,
    stream: &StreamInfo,
    tags: &Tags,
) -> Result<Box<dyn Muxer>, Error> {
    // All of this is checked before a byte is written.
    sample_entry::supports(stream.codec)?;
    let cookie = (!stream.extradata.is_empty()).then(|| stream.extradata.clone());
    if let Some(cookie) = &cookie {
        sample_entry::entry(stream, cookie)?;
    }
    sink.seek(SeekFrom::Current(0)).map_err(|e| Error::Unsupported {
        what: format!(
            "mp4: writing needs an output that can seek, to fill in the size of mdat at the \
             end ({e})"
        ),
    })?;
    let ilst = ilst::write(tags)?;
    let mut head = FTYP.to_vec();
    head.extend_from_slice(&mdat_header(0)?);
    sink.write_all(&head)?;
    Ok(Box::new(Mp4Muxer {
        sink,
        stream: stream.clone(),
        ilst,
        cookie,
        pos: DATA_AT,
        sizes: Vec::new(),
        durations: Vec::new(),
        start: None,
        next_pts: 0,
    }))
}

/// The 16 bytes after `ftyp` for `payload` bytes of audio: an empty `free` box and a plain
/// `mdat` header, or, when the size does not fit 32 bits, the 64-bit `mdat` header that takes
/// the room of both.
pub(super) fn mdat_header(payload: u64) -> Result<[u8; 16], Error> {
    let mut out = [0u8; 16];
    match u32::try_from(payload + 8) {
        Ok(size) => {
            out[..8].copy_from_slice(b"\0\0\0\x08free");
            out[8..12].copy_from_slice(&size.to_be_bytes());
            out[12..].copy_from_slice(b"mdat");
        }
        Err(_) => {
            out[..4].copy_from_slice(&1u32.to_be_bytes());
            out[4..8].copy_from_slice(b"mdat");
            out[8..].copy_from_slice(&(payload + 16).to_be_bytes());
        }
    }
    Ok(out)
}

/// The samples' places in the file: runs of consecutive samples, each a chunk.
#[derive(Default)]
pub(super) struct Chunks {
    offsets: Vec<u64>,
    counts: Vec<u32>,
}

impl Chunks {
    /// A new chunk starts once the open one holds `limit` bytes.
    pub(super) fn new(_limit: u64) -> Self {
        Self::default()
    }

    /// Notes a sample of `size` bytes at `offset`, which follows the previous one.
    pub(super) fn add(&mut self, _offset: u64, _size: u32) -> Result<(), Error> {
        Ok(())
    }

    /// The `stsc` box: runs of chunks with the same sample count.
    pub(super) fn stsc(&self) -> Result<Vec<u8>, Error> {
        Ok(Vec::new())
    }

    /// `stco`, or `co64` when an offset needs more than 32 bits.
    pub(super) fn offsets(&self) -> Result<Vec<u8>, Error> {
        Ok(Vec::new())
    }
}

fn be(version: u8, v: u64) -> Vec<u8> {
    if version == 1 {
        v.to_be_bytes().to_vec()
    } else {
        (v as u32).to_be_bytes().to_vec()
    }
}

const MATRIX: [u32; 9] = [0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000];

fn matrix() -> Vec<u8> {
    MATRIX.iter().flat_map(|v| v.to_be_bytes()).collect()
}

fn table(entries: u32, rows: &[u8]) -> Vec<u8> {
    let mut out = entries.to_be_bytes().to_vec();
    out.extend_from_slice(rows);
    out
}

impl Mp4Muxer {
    fn stbl(&self, entry: &[u8]) -> Result<Vec<u8>, Error> {
        let stsd = full(b"stsd", 0, 0, &[&table(1, entry)])?;
        let mut rows = Vec::new();
        for (n, d) in &self.durations {
            rows.extend_from_slice(&n.to_be_bytes());
            rows.extend_from_slice(&d.to_be_bytes());
        }
        let stts = full(b"stts", 0, 0, &[&table(self.durations.len() as u32, &rows)])?;
        let count = u32::try_from(self.sizes.len()).map_err(|_| too_big(*b"stsz"))?;
        let stsc = full(
            b"stsc",
            0,
            0,
            &[&table(
                1,
                &[
                    &1u32.to_be_bytes()[..],
                    &count.to_be_bytes(),
                    &1u32.to_be_bytes(),
                ]
                .concat(),
            )],
        )?;
        let mut sizes = 0u32.to_be_bytes().to_vec();
        sizes.extend_from_slice(&count.to_be_bytes());
        for s in &self.sizes {
            sizes.extend_from_slice(&s.to_be_bytes());
        }
        let stsz = full(b"stsz", 0, 0, &[&sizes])?;
        let stco = full(b"stco", 0, 0, &[&table(1, &(DATA_AT as u32).to_be_bytes())])?;
        boxed(b"stbl", &[&stsd, &stts, &stsc, &stsz, &stco])
    }

    /// The timeline the packets and the stream's length make; a length they cannot back is an
    /// error, not a short file.
    fn timeline(&self) -> Result<Timeline, Error> {
        let total: u64 = self
            .durations
            .iter()
            .map(|&(n, d)| u64::from(n) * u64::from(d))
            .sum();
        let priming = self.start.map_or(0, |s| s.unsigned_abs());
        let available = total.saturating_sub(priming);
        let audible = match self.stream.frames {
            Some(f) if f > available => {
                return Err(Error::Malformed {
                    what: format!(
                        "mp4: the stream is {f} frames long, but its packets hold {available} \
                         after {priming} of priming"
                    ),
                });
            }
            Some(f) => f,
            None => available,
        };
        Ok(Timeline {
            total,
            priming,
            audible,
        })
    }

    fn moov(&self, t: Timeline) -> Result<Vec<u8>, Error> {
        let rate = u64::from(self.stream.audio.sample_rate());
        let total = t.total;
        let version = u8::from(total > u64::from(u32::MAX));
        let times = [be(version, 0), be(version, 0)].concat();
        let mut mvhd = times.clone();
        mvhd.extend_from_slice(&(rate as u32).to_be_bytes());
        mvhd.extend_from_slice(&be(version, t.audible));
        mvhd.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        mvhd.extend_from_slice(&0x0100u16.to_be_bytes());
        mvhd.extend_from_slice(&[0; 10]);
        mvhd.extend_from_slice(&matrix());
        mvhd.extend_from_slice(&[0; 24]);
        mvhd.extend_from_slice(&2u32.to_be_bytes());
        let mvhd = full(b"mvhd", version, 0, &[&mvhd])?;

        let mut tkhd = times.clone();
        tkhd.extend_from_slice(&1u32.to_be_bytes());
        tkhd.extend_from_slice(&[0; 4]);
        tkhd.extend_from_slice(&be(version, t.audible));
        tkhd.extend_from_slice(&[0; 8]);
        tkhd.extend_from_slice(&[0; 4]);
        tkhd.extend_from_slice(&0x0100u16.to_be_bytes());
        tkhd.extend_from_slice(&[0; 2]);
        tkhd.extend_from_slice(&matrix());
        tkhd.extend_from_slice(&[0; 8]);
        let tkhd = full(b"tkhd", version, 7, &[&tkhd])?;

        let mut mdhd = times;
        mdhd.extend_from_slice(&(rate as u32).to_be_bytes());
        mdhd.extend_from_slice(&be(version, total));
        mdhd.extend_from_slice(&0x55C4u16.to_be_bytes());
        mdhd.extend_from_slice(&[0; 2]);
        let mdhd = full(b"mdhd", version, 0, &[&mdhd])?;
        let hdlr = full(b"hdlr", 0, 0, &[&[0; 4], b"soun", &[0; 12], b"\0"])?;

        let smhd = full(b"smhd", 0, 0, &[&[0; 4]])?;
        let url = full(b"url ", 0, 1, &[])?;
        let dref = full(b"dref", 0, 0, &[&table(1, &url)])?;
        let dinf = boxed(b"dinf", &[&dref])?;
        let cookie = self.cookie.as_deref().ok_or_else(|| Error::Malformed {
            what: "mp4: the alac track has no cookie: it comes with the stream's extradata or \
                   with set_extradata before finish"
                .into(),
        })?;
        let entry = sample_entry::entry(&self.stream, cookie)?;
        let minf = boxed(b"minf", &[&smhd, &dinf, &self.stbl(&entry)?])?;
        let mdia = boxed(b"mdia", &[&mdhd, &hdlr, &minf])?;
        let edts = if t.priming > 0 || t.audible < t.total {
            let mut elst = 1u32.to_be_bytes().to_vec();
            elst.extend_from_slice(&be(version, t.audible));
            elst.extend_from_slice(&be(version, t.priming));
            elst.extend_from_slice(&0x0001_0000u32.to_be_bytes());
            boxed(b"edts", &[&full(b"elst", version, 0, &[&elst])?])?
        } else {
            Vec::new()
        };
        let trak = boxed(b"trak", &[&tkhd, &edts, &mdia])?;
        let udta = if self.ilst.is_empty() {
            Vec::new()
        } else {
            let hdlr = full(b"hdlr", 0, 0, &[&[0; 4], b"mdirappl", &[0; 9]])?;
            let ilst = boxed(b"ilst", &[&self.ilst])?;
            boxed(b"udta", &[&full(b"meta", 0, 0, &[&hdlr, &ilst])?])?
        };
        boxed(b"moov", &[&mvhd, &trak, &udta])
    }
}

impl Muxer for Mp4Muxer {
    fn write_packet(&mut self, packet: &Packet) -> Result<(), Error> {
        if packet.stream != 0 {
            return Err(Error::Unsupported {
                what: format!(
                    "mp4: the writer holds one track, not stream {}",
                    packet.stream
                ),
            });
        }
        match self.start {
            None if packet.pts > 0 => {
                return Err(Error::Malformed {
                    what: format!(
                        "mp4: the first packet starts after 0 (pts {}): the timeline starts \
                         with the first frame meant to be heard",
                        packet.pts
                    ),
                });
            }
            Some(_) if packet.pts != self.next_pts => {
                return Err(Error::Malformed {
                    what: format!(
                        "mp4: a packet at pts {} does not follow the previous one, which ends \
                         at {}",
                        packet.pts, self.next_pts
                    ),
                });
            }
            _ => {}
        }
        let size = u32::try_from(packet.data.len()).map_err(|_| too_big(*b"mdat"))?;
        let duration = u32::try_from(packet.duration).map_err(|_| too_big(*b"stts"))?;
        self.sink.write_all(&packet.data)?;
        self.pos = self
            .pos
            .checked_add(u64::from(size))
            .ok_or_else(|| too_big(*b"mdat"))?;
        self.sizes.push(size);
        match self.durations.last_mut() {
            Some((n, d)) if *d == duration => *n += 1,
            _ => self.durations.push((1, duration)),
        }
        self.start.get_or_insert(packet.pts);
        self.next_pts = packet.pts + i64::from(duration);
        Ok(())
    }

    fn set_extradata(&mut self, extradata: &[u8]) -> Result<(), Error> {
        sample_entry::entry(&self.stream, extradata)?;
        self.cookie = Some(extradata.to_vec());
        Ok(())
    }

    fn finish(&mut self) -> Result<(), Error> {
        let moov = self.moov(self.timeline()?)?;
        self.sink.write_all(&moov)?;
        self.sink.seek(SeekFrom::Start(MDAT_AT))?;
        self.sink.write_all(&mdat_header(self.pos - DATA_AT)?)?;
        self.sink
            .seek(SeekFrom::Start(self.pos + moov.len() as u64))?;
        self.sink.finish()?;
        Ok(())
    }
}
