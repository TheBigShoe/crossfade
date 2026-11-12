//! iTunes-style metadata (`udta` > `meta` > `ilst`) onto the Vorbis-comment names the other
//! readers use.
//!
//! | Item | Tag |
//! |---|---|
//! | `©nam` | `TITLE` |
//! | `©ART` | `ARTIST` |
//! | `aART` | `ALBUMARTIST` |
//! | `©alb` | `ALBUM` |
//! | `©day` | `DATE` |
//! | `©gen`, `gnre` (ID3v1 genre number + 1) | `GENRE` |
//! | `trkn` | `TRACKNUMBER`, and `TRACKTOTAL` when the total is not 0 |
//! | `disk` | `DISCNUMBER`, and `DISCTOTAL` when the total is not 0 |
//! | `©cmt` | `COMMENT` |
//! | `©wrt` | `COMPOSER` |
//! | `©too` | `ENCODER` |
//! | `----` (free form) | its `name`, as written |
//!
//! Other items, cover art (`covr`) among them, are skipped. The free-form `iTunSMPB` is the
//! encoder's delay and padding, not a tag: the reader takes it for the timeline instead.

use super::boxes::{Children, Cursor, FourCc};
use crossfade_core::{Error, Tags, tag_keys};

/// The ID3v1 genres (0-79), which `gnre` numbers from 1.
const GENRES: [&str; 80] = [
    "Blues",
    "Classic Rock",
    "Country",
    "Dance",
    "Disco",
    "Funk",
    "Grunge",
    "Hip-Hop",
    "Jazz",
    "Metal",
    "New Age",
    "Oldies",
    "Other",
    "Pop",
    "R&B",
    "Rap",
    "Reggae",
    "Rock",
    "Techno",
    "Industrial",
    "Alternative",
    "Ska",
    "Death Metal",
    "Pranks",
    "Soundtrack",
    "Euro-Techno",
    "Ambient",
    "Trip-Hop",
    "Vocal",
    "Jazz+Funk",
    "Fusion",
    "Trance",
    "Classical",
    "Instrumental",
    "Acid",
    "House",
    "Game",
    "Sound Clip",
    "Gospel",
    "Noise",
    "AlternRock",
    "Bass",
    "Soul",
    "Punk",
    "Space",
    "Meditative",
    "Instrumental Pop",
    "Instrumental Rock",
    "Ethnic",
    "Gothic",
    "Darkwave",
    "Techno-Industrial",
    "Electronic",
    "Pop-Folk",
    "Eurodance",
    "Dream",
    "Southern Rock",
    "Comedy",
    "Cult",
    "Gangsta",
    "Top 40",
    "Christian Rap",
    "Pop/Funk",
    "Jungle",
    "Native American",
    "Cabaret",
    "New Wave",
    "Psychadelic",
    "Rave",
    "Showtunes",
    "Trailer",
    "Lo-Fi",
    "Tribal",
    "Acid Punk",
    "Acid Jazz",
    "Polka",
    "Retro",
    "Musical",
    "Rock & Roll",
    "Hard Rock",
];

/// What `ilst` gave: the tags, and the raw `iTunSMPB` value if there was one.
#[derive(Debug, Default)]
pub(super) struct Metadata {
    pub(super) tags: Tags,
    pub(super) smpb: Option<String>,
}

fn text_key(item: &FourCc) -> Option<&'static str> {
    Some(match item {
        b"\xA9nam" => tag_keys::TITLE,
        b"\xA9ART" => tag_keys::ARTIST,
        b"aART" => "ALBUMARTIST",
        b"\xA9alb" => tag_keys::ALBUM,
        b"\xA9day" => tag_keys::DATE,
        b"\xA9gen" => tag_keys::GENRE,
        b"\xA9cmt" => tag_keys::COMMENT,
        b"\xA9wrt" => "COMPOSER",
        b"\xA9too" => "ENCODER",
        _ => return None,
    })
}

/// The payload of each `data` box in an item: (type, bytes).
fn data(item: &[u8]) -> impl Iterator<Item = (u32, &[u8])> {
    Children::new(item).filter_map(|child| {
        let (kind, body) = child.ok()?;
        if &kind != b"data" || body.len() < 8 {
            return None;
        }
        let kind = u32::from_be_bytes([body[0], body[1], body[2], body[3]]) & 0xFF_FFFF;
        Some((kind, &body[8..]))
    })
}

/// UTF-8 (type 1) or UTF-16 big-endian (type 2) text; a damaged byte becomes U+FFFD.
fn text(kind: u32, bytes: &[u8]) -> Option<String> {
    let s = match kind {
        1 => String::from_utf8_lossy(bytes).into_owned(),
        2 => {
            let units: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        }
        _ => return None,
    };
    let s = s.trim_end_matches('\0');
    (!s.is_empty()).then(|| s.to_owned())
}

/// `trkn` and `disk`: two reserved bytes, the number and the total, as 16-bit integers.
fn pair(tags: &mut Tags, bytes: &[u8], number: &str, total: &str) {
    let mut c = Cursor::new(bytes, "trkn");
    let (Ok(_), Ok(n), Ok(of)) = (c.u16(), c.u16(), c.u16()) else {
        return;
    };
    if n != 0 {
        tags.push(number, n.to_string());
    }
    if of != 0 {
        tags.push(total, of.to_string());
    }
}

/// A free-form item: `mean`, `name`, then `data`.
fn free_form(meta: &mut Metadata, item: &[u8]) {
    let mut name = None;
    for (kind, body) in Children::new(item).flatten() {
        if &kind == b"name" && body.len() >= 4 {
            name = Some(String::from_utf8_lossy(&body[4..]).into_owned());
        }
    }
    let Some(name) = name.filter(|n| !n.is_empty()) else {
        return;
    };
    for (kind, bytes) in data(item) {
        let Some(value) = text(kind, bytes) else {
            continue;
        };
        if name == "iTunSMPB" {
            meta.smpb.get_or_insert(value);
        } else {
            meta.tags.push(name.clone(), value);
        }
    }
}

/// Reads an `ilst` body. Damaged items are skipped: the audio plays without them.
pub(super) fn parse(ilst: &[u8]) -> Metadata {
    let mut meta = Metadata::default();
    for (item, body) in Children::new(ilst).flatten() {
        if let Some(key) = text_key(&item) {
            for (kind, bytes) in data(body) {
                if let Some(value) = text(kind, bytes) {
                    meta.tags.push(key, value);
                }
            }
            continue;
        }
        match &item {
            b"trkn" => {
                if let Some((_, bytes)) = data(body).next() {
                    pair(&mut meta.tags, bytes, tag_keys::TRACKNUMBER, "TRACKTOTAL");
                }
            }
            b"disk" => {
                if let Some((_, bytes)) = data(body).next() {
                    pair(&mut meta.tags, bytes, "DISCNUMBER", "DISCTOTAL");
                }
            }
            b"gnre" => {
                if let Some((_, &[hi, lo, ..])) = data(body).next() {
                    let n = usize::from(u16::from_be_bytes([hi, lo]));
                    if let Some(name) = n.checked_sub(1).and_then(|i| GENRES.get(i)) {
                        meta.tags.push(tag_keys::GENRE, *name);
                    }
                }
            }
            b"----" => free_form(&mut meta, body),
            _ => {}
        }
    }
    meta
}

/// The `ilst` inside a `meta` body, which is a full box in MP4 and a plain one in QuickTime.
pub(super) fn in_meta(meta: &[u8]) -> Result<Option<&[u8]>, Error> {
    let children = match meta.get(4..8) {
        Some(b"hdlr") => meta,
        _ => meta.get(4..).unwrap_or_default(),
    };
    super::boxes::find(children, b"ilst")
}
