//! The MP4/M4A writer, for ALAC: `ftyp`, then `mdat` (the packets, as they come), then
//! `moov` once every size and duration is known, so nothing is held but a size and a
//! duration per packet. The sink is sought once at the end, to write `mdat`'s length.
//!
//! One sound track, timed in samples (the media timescale is the sample rate), the packets
//! four to a chunk. ALAC has no priming, so there is no edit list: the track is as long as its
//! packets say. Tags are written as iTunes items (`udta` > `meta` > `ilst`), the inverse of
//! what the reader maps (`ilst.rs`); a tag with no item of its own becomes a free-form one.

use crossfade_core::{CodecId, Error, MediaSink, Muxer, Packet, StreamInfo, Tags, tag_keys};
use std::io::SeekFrom;

/// Packets per chunk: few offsets to store, and a reader still finds its place quickly.
const PER_CHUNK: usize = 4;

fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    out
}

/// A box with a version and flags.
fn full(kind: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut b = vec![version];
    b.extend_from_slice(&flags.to_be_bytes()[1..]);
    b.extend_from_slice(body);
    bx(kind, &b)
}

fn unsupported(what: impl Into<String>) -> Error {
    Error::Unsupported { what: what.into() }
}

struct Mp4Muxer {
    sink: Box<dyn MediaSink>,
    config: crossfade_alac::Config,
    cookie: Vec<u8>,
    sample_rate: u32,
    channels: u16,
    tags: Tags,
    /// Where `mdat`'s 64-bit length goes, and where its data begins.
    mdat_size_at: u64,
    data_start: u64,
    data_len: u64,
    /// Per packet: bytes and frames.
    sizes: Vec<u32>,
    durations: Vec<u32>,
}

pub(crate) fn create(
    mut sink: Box<dyn MediaSink>,
    stream: &StreamInfo,
    tags: &Tags,
) -> Result<Box<dyn Muxer>, Error> {
    if stream.codec != CodecId::Alac {
        return Err(unsupported(format!(
            "mp4 files hold alac here, not {}",
            stream.codec.name()
        )));
    }
    let config =
        crossfade_alac::Config::parse(&stream.extradata).map_err(|e| unsupported(e.to_string()))?;
    let ftyp = bx(b"ftyp", b"M4A \0\0\0\0M4A mp42isom\0\0\0\0");
    sink.write_all(&ftyp)?;
    // A 64-bit length, so that a long file needs no second layout.
    sink.write_all(&[0, 0, 0, 1])?;
    sink.write_all(b"mdat")?;
    sink.write_all(&[0; 8])?;
    let mdat_size_at = ftyp.len() as u64 + 8;
    Ok(Box::new(Mp4Muxer {
        sink,
        config,
        cookie: config.write().to_vec(),
        sample_rate: stream.audio.sample_rate(),
        channels: stream.audio.layout().channels(),
        tags: tags.clone(),
        mdat_size_at,
        data_start: mdat_size_at + 8,
        data_len: 0,
        sizes: Vec::new(),
        durations: Vec::new(),
    }))
}

impl Mp4Muxer {
    fn stbl(&self) -> Vec<u8> {
        // The sample entry (version 0) around the cookie.
        let mut entry = vec![0; 6];
        entry.extend_from_slice(&1u16.to_be_bytes());
        entry.extend_from_slice(&[0; 8]);
        entry.extend_from_slice(&self.channels.to_be_bytes());
        entry.extend_from_slice(&u16::from(self.config.bit_depth).to_be_bytes());
        entry.extend_from_slice(&[0; 4]);
        // 16.16: a rate past 65535 Hz does not fit, and the cookie has it whole.
        entry.extend_from_slice(&(self.sample_rate.min(0xFFFF) << 16).to_be_bytes());
        entry.extend_from_slice(&full(b"alac", 0, 0, &self.cookie));
        let mut stsd = 1u32.to_be_bytes().to_vec();
        stsd.extend_from_slice(&bx(b"alac", &entry));

        let mut runs: Vec<(u32, u32)> = Vec::new();
        for &d in &self.durations {
            match runs.last_mut() {
                Some((n, delta)) if *delta == d => *n += 1,
                _ => runs.push((1, d)),
            }
        }
        let mut stts = (runs.len() as u32).to_be_bytes().to_vec();
        for (n, d) in runs {
            stts.extend_from_slice(&n.to_be_bytes());
            stts.extend_from_slice(&d.to_be_bytes());
        }

        let chunks: Vec<&[u32]> = self.sizes.chunks(PER_CHUNK).collect();
        // Every chunk holds PER_CHUNK packets but perhaps the last.
        let mut stsc_entries: Vec<(u32, u32)> = Vec::new();
        if !chunks.is_empty() {
            stsc_entries.push((1, chunks[0].len() as u32));
            let last = chunks[chunks.len() - 1].len() as u32;
            if chunks.len() > 1 && last != PER_CHUNK as u32 {
                stsc_entries.push((chunks.len() as u32, last));
            }
        }
        let mut stsc = (stsc_entries.len() as u32).to_be_bytes().to_vec();
        for (first, per) in stsc_entries {
            stsc.extend_from_slice(&first.to_be_bytes());
            stsc.extend_from_slice(&per.to_be_bytes());
            stsc.extend_from_slice(&1u32.to_be_bytes());
        }

        let mut stsz = 0u32.to_be_bytes().to_vec();
        stsz.extend_from_slice(&(self.sizes.len() as u32).to_be_bytes());
        for s in &self.sizes {
            stsz.extend_from_slice(&s.to_be_bytes());
        }

        let wide = self.data_start + self.data_len > u64::from(u32::MAX);
        let mut offsets = (chunks.len() as u32).to_be_bytes().to_vec();
        let mut at = self.data_start;
        for c in &chunks {
            if wide {
                offsets.extend_from_slice(&at.to_be_bytes());
            } else {
                offsets.extend_from_slice(&(at as u32).to_be_bytes());
            }
            at += c.iter().map(|&s| u64::from(s)).sum::<u64>();
        }
        let stco = full(if wide { b"co64" } else { b"stco" }, 0, 0, &offsets);
        bx(
            b"stbl",
            &[
                full(b"stsd", 0, 0, &stsd),
                full(b"stts", 0, 0, &stts),
                full(b"stsc", 0, 0, &stsc),
                full(b"stsz", 0, 0, &stsz),
                stco,
            ]
            .concat(),
        )
    }

    fn moov(&self) -> Vec<u8> {
        let frames: u64 = self.durations.iter().map(|&d| u64::from(d)).sum();
        // Version 0 holds 32 bits of duration: 27 hours at 44.1 kHz. Past that the tables
        // still say how long the track is.
        let duration = u32::try_from(frames).unwrap_or(u32::MAX);
        const UNITY: [u8; 36] = [
            0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0x40, 0, 0, 0,
        ];
        let mut mvhd = vec![0; 8];
        mvhd.extend_from_slice(&self.sample_rate.to_be_bytes());
        mvhd.extend_from_slice(&duration.to_be_bytes());
        mvhd.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // rate 1.0
        mvhd.extend_from_slice(&0x0100u16.to_be_bytes()); // volume 1.0
        mvhd.extend_from_slice(&[0; 10]);
        mvhd.extend_from_slice(&UNITY);
        mvhd.extend_from_slice(&[0; 24]);
        mvhd.extend_from_slice(&2u32.to_be_bytes()); // next track id

        let mut tkhd = vec![0; 8];
        tkhd.extend_from_slice(&1u32.to_be_bytes()); // track id
        tkhd.extend_from_slice(&[0; 4]);
        tkhd.extend_from_slice(&duration.to_be_bytes());
        tkhd.extend_from_slice(&[0; 12]);
        tkhd.extend_from_slice(&0x0100u16.to_be_bytes());
        tkhd.extend_from_slice(&[0; 2]);
        tkhd.extend_from_slice(&UNITY);
        tkhd.extend_from_slice(&[0; 8]);

        let mut mdhd = vec![0; 8];
        mdhd.extend_from_slice(&self.sample_rate.to_be_bytes());
        mdhd.extend_from_slice(&duration.to_be_bytes());
        mdhd.extend_from_slice(&0x55C4u16.to_be_bytes()); // language: undetermined
        mdhd.extend_from_slice(&[0; 2]);

        let mut hdlr = vec![0; 4];
        hdlr.extend_from_slice(b"soun");
        hdlr.extend_from_slice(&[0; 12]);
        hdlr.extend_from_slice(b"SoundHandler\0");

        let url = full(b"url ", 0, 1, &[]); // the media is in this file
        let mut dref = 1u32.to_be_bytes().to_vec();
        dref.extend_from_slice(&url);
        let minf = bx(
            b"minf",
            &[
                full(b"smhd", 0, 0, &[0; 4]),
                bx(b"dinf", &full(b"dref", 0, 0, &dref)),
                self.stbl(),
            ]
            .concat(),
        );
        let mdia = bx(
            b"mdia",
            &[full(b"mdhd", 0, 0, &mdhd), full(b"hdlr", 0, 0, &hdlr), minf].concat(),
        );
        // Flags: enabled, in the movie, in the preview.
        let trak = bx(b"trak", &[full(b"tkhd", 0, 7, &tkhd), mdia].concat());
        bx(
            b"moov",
            &[full(b"mvhd", 0, 0, &mvhd), trak, self.udta()].concat(),
        )
    }

    /// The tags as iTunes items; nothing at all when there are none.
    fn udta(&self) -> Vec<u8> {
        if self.tags.is_empty() {
            return Vec::new();
        }
        let data = |kind: u32, payload: &[u8]| {
            let mut b = kind.to_be_bytes().to_vec();
            b.extend_from_slice(&[0; 4]);
            b.extend_from_slice(payload);
            bx(b"data", &b)
        };
        let number = |key: &str| {
            self.tags
                .get(key)
                .and_then(|v| v.trim().parse::<u16>().ok())
        };
        let mut items = Vec::new();
        let mut pair = |kind: &[u8; 4], num: &str, total: &str, tail: usize| {
            let (n, of) = (number(num), number(total));
            if n.is_some() || of.is_some() {
                let mut b = vec![0, 0];
                b.extend_from_slice(&n.unwrap_or(0).to_be_bytes());
                b.extend_from_slice(&of.unwrap_or(0).to_be_bytes());
                b.extend_from_slice(&vec![0; tail]);
                items.extend_from_slice(&bx(kind, &data(0, &b)));
            }
            (n.is_some(), of.is_some())
        };
        let track = pair(b"trkn", tag_keys::TRACKNUMBER, "TRACKTOTAL", 2);
        let disc = pair(b"disk", "DISCNUMBER", "DISCTOTAL", 0);
        for tag in self.tags.iter() {
            let item: Option<&[u8; 4]> = match tag.key.as_str() {
                tag_keys::TITLE => Some(b"\xA9nam"),
                tag_keys::ARTIST => Some(b"\xA9ART"),
                "ALBUMARTIST" => Some(b"aART"),
                tag_keys::ALBUM => Some(b"\xA9alb"),
                tag_keys::DATE => Some(b"\xA9day"),
                tag_keys::GENRE => Some(b"\xA9gen"),
                tag_keys::COMMENT => Some(b"\xA9cmt"),
                "COMPOSER" => Some(b"\xA9wrt"),
                "ENCODER" => Some(b"\xA9too"),
                _ => None,
            };
            // The numbered pairs went out above, when they were numbers.
            let in_pair = match tag.key.as_str() {
                tag_keys::TRACKNUMBER => track.0,
                "TRACKTOTAL" => track.1,
                "DISCNUMBER" => disc.0,
                "DISCTOTAL" => disc.1,
                _ => false,
            };
            if in_pair || tag.value.is_empty() {
                continue;
            }
            let value = data(1, tag.value.as_bytes());
            match item {
                Some(kind) => items.extend_from_slice(&bx(kind, &value)),
                None => {
                    let free = [
                        full(b"mean", 0, 0, b"com.apple.iTunes"),
                        full(b"name", 0, 0, tag.key.as_bytes()),
                        value,
                    ]
                    .concat();
                    items.extend_from_slice(&bx(b"----", &free));
                }
            }
        }
        let mut hdlr = vec![0; 4];
        hdlr.extend_from_slice(b"mdirappl");
        hdlr.extend_from_slice(&[0; 9]);
        let meta = full(
            b"meta",
            0,
            0,
            &[full(b"hdlr", 0, 0, &hdlr), bx(b"ilst", &items)].concat(),
        );
        bx(b"udta", &meta)
    }
}

impl Muxer for Mp4Muxer {
    fn write_packet(&mut self, packet: &Packet) -> Result<(), Error> {
        let size = u32::try_from(packet.data.len()).map_err(|_| Error::Overflow)?;
        let duration = u32::try_from(packet.duration).map_err(|_| Error::Overflow)?;
        self.sink.write_all(&packet.data)?;
        self.data_len += u64::from(size);
        self.sizes.push(size);
        self.durations.push(duration);
        Ok(())
    }

    fn finish(&mut self) -> Result<(), Error> {
        let moov = self.moov();
        self.sink.write_all(&moov)?;
        self.sink.seek(SeekFrom::Start(self.mdat_size_at))?;
        self.sink.write_all(&(16 + self.data_len).to_be_bytes())?;
        self.sink.finish()?;
        Ok(())
    }
}
