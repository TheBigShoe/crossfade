//! The reader: finds `moov`, then hands out one MP4 sample per packet through the tables.

use super::boxes::{fill_exact, malformed, next_top, read_body, skip_to};
use super::moov::{self, Edit, Movie};
use super::table::SampleTable;
use crossfade_core::{
    AudioSpec, CodecId, Demuxer, Error, Limits, MediaSource, Packet, StreamInfo, Tags, TimeBase,
};
use std::io::SeekFrom;

/// Media time to sample frames: `t * rate / timescale`, rounded down. Every position on the
/// timeline goes through this one function, so a packet's duration is the difference of two
/// rounded positions and no rounding error accumulates.
#[derive(Debug, Clone, Copy)]
struct Clock {
    rate: u32,
    timescale: u32,
}

impl Clock {
    fn frames(self, t: u64) -> u64 {
        if self.rate == self.timescale {
            return t;
        }
        let f = u128::from(t) * u128::from(self.rate) / u128::from(self.timescale);
        u64::try_from(f).unwrap_or(u64::MAX)
    }
}

/// Where the timeline starts and how long it plays: priming frames (before pts 0) and the
/// audible length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Gapless {
    pub(super) priming: u64,
    pub(super) frames: u64,
}

/// The iTunes gapless tag: hex fields, of which the second is the priming, the third the
/// padding and the fourth the original length in frames.
fn smpb(value: &str) -> Option<(u64, u64)> {
    let fields: Vec<u64> = value
        .split_whitespace()
        .take(4)
        .map(|f| u64::from_str_radix(f, 16).ok())
        .collect::<Option<_>>()?;
    match fields[..] {
        [_, priming, _padding, length] => Some((priming, length)),
        _ => None,
    }
}

/// The timeline from, in this order of trust: the edit list, then `iTunSMPB`, then nothing
/// (the whole track plays). The edit list comes first because it is the container's own
/// statement of what to play, which every ISO reader honours and every remux keeps in step
/// with the samples; `iTunSMPB` is an iTunes tag that tools copy along without checking. When
/// they disagree, the edit list wins. A source whose priming lies past the end of the track is
/// not believed, and no audible length runs past the end.
pub(super) fn gapless(
    edit: Option<Edit>,
    smpb_tag: Option<&str>,
    total: u64,
    media: impl Fn(u64) -> u64,
    movie: impl Fn(u64) -> u64,
) -> Gapless {
    if let Some(e) = edit {
        let priming = media(e.media_time);
        if priming <= total {
            let rest = total - priming;
            let frames = if e.segment_duration == 0 {
                rest
            } else {
                movie(e.segment_duration).min(rest)
            };
            return Gapless { priming, frames };
        }
    }
    if let Some((priming, length)) = smpb_tag.and_then(smpb)
        && priming <= total
    {
        return Gapless {
            priming,
            frames: length.min(total - priming),
        };
    }
    Gapless {
        priming: 0,
        frames: total,
    }
}

/// Packets to go back so a decoder has its pre-roll: ALAC packets stand alone and an AAC frame
/// overlaps the one before it. MP3 is not fixed: its bit reservoir reaches back by bytes, so
/// `Mp4Demuxer::seek` counts frame sizes.
fn preroll(codec: CodecId) -> u64 {
    match codec {
        CodecId::Aac => 1,
        _ => 0,
    }
}

/// Bytes of main data in an MP3 sample of `size` bytes: its header (with a CRC, which only
/// makes the guess smaller and so the pre-roll longer) and side information are not.
fn mp3_main_data(size: u64, lsf: bool, mono: bool) -> usize {
    let side_info = match (lsf, mono) {
        (false, true) => 17,
        (false, false) => 32,
        (true, true) => 9,
        (true, false) => 17,
    };
    usize::try_from(size.saturating_sub(4 + 2 + side_info)).unwrap_or(usize::MAX)
}

struct Mp4Demuxer {
    source: Box<dyn MediaSource>,
    pos: u64,
    streams: [StreamInfo; 1],
    tags: Tags,
    table: SampleTable,
    clock: Clock,
    priming: i64,
    preroll: u64,
    /// Set for MP3: (MPEG-2 or 2.5, mono), which the reservoir arithmetic needs.
    mp3: Option<(bool, bool)>,
    next: u64,
    max_packet: usize,
}

impl Mp4Demuxer {
    fn pts(&self, sample: u64) -> i64 {
        // `open` checked that the end of the track fits.
        self.clock.frames(self.table.time(sample)) as i64 - self.priming
    }
}

impl Demuxer for Mp4Demuxer {
    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }

    fn tags(&self) -> &Tags {
        &self.tags
    }

    fn read_packet(&mut self, out: &mut Packet) -> Result<bool, Error> {
        if self.next >= self.table.count() {
            return Ok(false);
        }
        let (offset, size) = self.table.locate(self.next)?;
        let size = usize::try_from(size)
            .ok()
            .filter(|&s| s <= self.max_packet)
            .ok_or(Error::AllocLimitExceeded {
                requested: usize::try_from(size).unwrap_or(usize::MAX),
                max: self.max_packet,
            })?;
        if offset < self.pos {
            if !self.source.is_seekable() {
                return Err(malformed(
                    "samples out of file order need an input that can seek",
                ));
            }
            self.source.seek(SeekFrom::Start(offset))?;
            self.pos = offset;
        } else {
            skip_to(self.source.as_mut(), &mut self.pos, offset)?;
        }
        out.data.clear();
        out.data.resize(size, 0);
        if let Err(e) = fill_exact(self.source.as_mut(), &mut self.pos, &mut out.data) {
            out.data.clear();
            return Err(e);
        }
        out.stream = 0;
        out.pts = self.pts(self.next);
        out.duration = (self.pts(self.next + 1) - out.pts) as u64;
        self.next += 1;
        Ok(true)
    }

    fn seek(&mut self, pts: i64) -> Result<i64, Error> {
        if !self.source.is_seekable() {
            return Err(Error::Unsupported {
                what: "mp4: this input cannot seek".into(),
            });
        }
        let count = self.table.count();
        let end = self.pts(count);
        if pts >= end {
            self.next = count;
            return Ok(end);
        }
        let (clock, priming) = (self.clock, self.priming);
        let at = self
            .table
            .sample_at(pts, |t| clock.frames(t) as i64 - priming);
        self.next = match self.mp3 {
            Some((lsf, mono)) => crate::mp3::first_frame_to_decode(at as usize, lsf, |i| {
                mp3_main_data(self.table.size(i as u64), lsf, mono)
            }) as u64,
            None => at.saturating_sub(self.preroll),
        };
        Ok(self.pts(self.next))
    }
}

fn find_moov(
    source: &mut dyn MediaSource,
    pos: &mut u64,
    limits: &Limits,
) -> Result<Vec<u8>, Error> {
    let no_moov = || malformed("no moov box");
    loop {
        let Some(b) = next_top(source, pos)? else {
            return Err(no_moov());
        };
        match &b.kind {
            b"moov" => return read_body(source, pos, b.body_len, limits.max_alloc_bytes),
            b"moof" => {
                return Err(Error::Unsupported {
                    what: "fragmented mp4 is not supported yet".into(),
                });
            }
            b"mdat" if !source.is_seekable() => {
                return Err(Error::Unsupported {
                    what: "mp4: this file keeps its index (moov) after the audio (mdat), and \
                           reading it needs an input that can seek, which this one cannot \
                           (a pipe?)"
                        .into(),
                });
            }
            _ => {}
        }
        let len = b.body_len.ok_or_else(no_moov)?;
        let end = b
            .body_start
            .checked_add(len)
            .ok_or_else(|| malformed("box size out of range"))?;
        skip_to(source, pos, end)?;
    }
}

pub(super) fn open(
    mut source: Box<dyn MediaSource>,
    limits: &Limits,
) -> Result<Box<dyn Demuxer>, Error> {
    let mut pos = 0;
    let moov = find_moov(source.as_mut(), &mut pos, limits)?;
    let Movie {
        track,
        movie_timescale,
        meta,
    } = moov::parse(&moov, limits)?;
    let entry = track.entry;
    let rate = if entry.sample_rate != 0 {
        entry.sample_rate
    } else {
        track.timescale
    };
    let clock = Clock {
        rate,
        timescale: track.timescale,
    };
    let total = clock.frames(track.table.end_time());
    if i64::try_from(total).is_err() {
        return Err(malformed("the track is too long"));
    }
    let movie_clock = Clock {
        rate,
        timescale: movie_timescale.max(1),
    };
    let movie = |d: u64| {
        // A length rounds to the nearest frame: a coarse movie timescale (600, 1000) says it to
        // a fraction of a frame.
        let (r, ts) = (u128::from(rate), u128::from(movie_clock.timescale));
        u64::try_from((u128::from(d) * r * 2 + ts) / (2 * ts)).unwrap_or(u64::MAX)
    };
    let g = gapless(
        track.edit,
        meta.smpb.as_deref(),
        total,
        |t| clock.frames(t),
        movie,
    );
    let channels = entry.layout.channels();
    let audio = AudioSpec::new(rate, entry.format, entry.layout)?;
    let stream = StreamInfo {
        frames: Some(g.frames),
        extradata: entry.extradata,
        bits_per_raw_sample: entry.bits_per_raw_sample,
        ..StreamInfo::new(0, entry.codec, audio, TimeBase::new(1, i64::from(rate))?)
    };
    Ok(Box::new(Mp4Demuxer {
        source,
        pos,
        streams: [stream],
        tags: meta.tags,
        preroll: preroll(entry.codec),
        mp3: (entry.codec == CodecId::Mp3).then(|| (rate < 32000, channels == 1)),
        table: track.table,
        clock,
        priming: g.priming as i64,
        next: 0,
        max_packet: limits.max_packet_bytes,
    }))
}
