//! The movie box: the first sound track's description and tables, the movie's edit list and
//! its iTunes metadata.

use super::boxes::{Children, Cursor, find, malformed};
use super::entry::{self, Entry};
use super::ilst::{self, Metadata};
use super::table::{Boxes, SampleTable};
use crossfade_core::{Error, Limits};

/// One edit (`elst`): play the media from `media_time` (media timescale) for
/// `segment_duration` (movie timescale; 0 when the writer left it open).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Edit {
    pub(super) media_time: u64,
    pub(super) segment_duration: u64,
}

pub(super) struct Track {
    pub(super) entry: Entry,
    pub(super) timescale: u32,
    pub(super) table: SampleTable,
    pub(super) edit: Option<Edit>,
}

pub(super) struct Movie {
    pub(super) track: Track,
    pub(super) movie_timescale: u32,
    pub(super) meta: Metadata,
}

fn fragmented() -> Error {
    Error::Unsupported {
        what: "fragmented mp4 is not supported yet".into(),
    }
}

/// One level deeper, within the nesting limit.
fn deeper(depth: usize, limits: &Limits) -> Result<usize, Error> {
    let d = depth + 1;
    if d > limits.max_nesting_depth {
        return Err(malformed("boxes nest deeper than the limit"));
    }
    Ok(d)
}

fn child<'a>(body: &'a [u8], kind: &[u8; 4], parent: &str) -> Result<&'a [u8], Error> {
    find(body, kind)?.ok_or_else(|| {
        malformed(format!(
            "'{parent}' has no '{}'",
            String::from_utf8_lossy(kind)
        ))
    })
}

/// The timescale field of `mvhd` or `mdhd` (they share the layout up to it).
fn timescale(body: &[u8], what: &'static str) -> Result<u32, Error> {
    let mut c = Cursor::new(body, what);
    let version = c.version()?;
    c.skip(if version == 1 { 16 } else { 8 })?;
    c.u32()
}

/// The edit list, when it is the one form a gapless file uses: a single edit at normal rate
/// that starts in the media. Anything else (empty edits, several segments, other rates) is
/// ignored and the track plays whole.
fn edit(elst: &[u8]) -> Result<Option<Edit>, Error> {
    let mut c = Cursor::new(elst, "elst");
    let version = c.version()?;
    let n = c.u32()?;
    if n != 1 {
        return Ok(None);
    }
    let (segment_duration, media_time) = if version == 1 {
        (c.u64()?, c.u64()? as i64)
    } else {
        (u64::from(c.u32()?), i64::from(c.u32()? as i32))
    };
    let rate = c.u32()?;
    if media_time < 0 || rate != 0x0001_0000 {
        return Ok(None);
    }
    Ok(Some(Edit {
        media_time: media_time as u64,
        segment_duration,
    }))
}

/// The track if it is a sound track, else `None`.
fn track(trak: &[u8], depth: usize, limits: &Limits) -> Result<Option<Track>, Error> {
    let depth = deeper(depth, limits)?;
    let mdia = child(trak, b"mdia", "trak")?;
    let inner = deeper(depth, limits)?;
    let hdlr = child(mdia, b"hdlr", "mdia")?;
    if hdlr.get(8..12) != Some(b"soun") {
        return Ok(None);
    }
    let timescale = timescale(child(mdia, b"mdhd", "mdia")?, "mdhd")?;
    if timescale == 0 {
        return Err(malformed("the track's timescale is 0"));
    }
    let edit = match find(trak, b"edts")? {
        Some(edts) => match find(edts, b"elst")? {
            Some(elst) => edit(elst)?,
            None => None,
        },
        None => None,
    };
    let minf = child(mdia, b"minf", "mdia")?;
    let inner = deeper(inner, limits)?;
    let stbl = child(minf, b"stbl", "minf")?;
    let _ = deeper(inner, limits)?;
    let mut boxes = Boxes::default();
    let mut stsd = None;
    for c in Children::new(stbl) {
        let (kind, body) = c?;
        let slot = match &kind {
            b"stsd" => &mut stsd,
            b"stts" => &mut boxes.stts,
            b"stsc" => &mut boxes.stsc,
            b"stsz" => &mut boxes.stsz,
            b"stz2" => &mut boxes.stz2,
            b"stco" => &mut boxes.stco,
            b"co64" => &mut boxes.co64,
            _ => continue,
        };
        slot.get_or_insert(body);
    }
    let entry = entry::parse(stsd.ok_or_else(|| malformed("'stbl' has no 'stsd'"))?)?;
    let table = SampleTable::new(&boxes, limits)?;
    Ok(Some(Track {
        entry,
        timescale,
        table,
        edit,
    }))
}

pub(super) fn parse(moov: &[u8], limits: &Limits) -> Result<Movie, Error> {
    let depth = deeper(0, limits)?;
    let mut movie_timescale = 0;
    let mut sound = None;
    let mut meta = None;
    for c in Children::new(moov) {
        let (kind, body) = c?;
        match &kind {
            b"mvhd" => movie_timescale = timescale(body, "mvhd")?,
            b"mvex" => return Err(fragmented()),
            b"trak" if sound.is_none() => sound = track(body, depth, limits)?,
            b"udta" if meta.is_none() => {
                deeper(depth, limits)?;
                if let Some(m) = find(body, b"meta")? {
                    deeper(deeper(depth, limits)?, limits)?;
                    meta = ilst::in_meta(m)?.map(ilst::parse);
                }
            }
            b"meta" if meta.is_none() => {
                deeper(depth, limits)?;
                meta = ilst::in_meta(body)?.map(ilst::parse);
            }
            _ => {}
        }
    }
    let track = sound.ok_or_else(|| Error::Unsupported {
        what: "mp4: the file has no audio track".into(),
    })?;
    Ok(Movie {
        track,
        movie_timescale,
        meta: meta.unwrap_or_default(),
    })
}
