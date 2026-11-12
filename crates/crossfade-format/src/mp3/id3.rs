//! ID3 tags: v2.2, v2.3 and v2.4 in front of the audio, v1 (and an APEv2 tag, skipped) at the
//! end, mapped onto the Vorbis-comment keys the other readers use.
//!
//! Written from the id3.org specifications. Every length is checked against the bytes that
//! are really there, so a hostile tag costs at most a few times its own size.

use crossfade_core::{Tags, tag_keys};

/// Entries kept from one tag, so a tag of a million empty frames stays cheap.
const MAX_ENTRIES: usize = 1024;

/// Appends the text frames of every ID3v2 tag in `bytes` (tags back to back) to `tags`.
pub(super) fn parse_v2(mut bytes: &[u8], tags: &mut Tags) {
    while bytes.len() >= 10 && bytes.starts_with(b"ID3") {
        let size = syncsafe(&bytes[6..10]) as usize;
        let footer = if bytes[5] & 0x10 != 0 { 10 } else { 0 };
        let body_end = (10 + size).min(bytes.len());
        parse_one(bytes[3], bytes[5], &bytes[10..body_end], tags);
        bytes = &bytes[(10 + size + footer).min(bytes.len())..];
    }
}

fn syncsafe(b: &[u8]) -> u32 {
    b.iter().fold(0, |acc, &x| acc << 7 | u32::from(x & 0x7F))
}

fn be(b: &[u8]) -> u32 {
    b.iter().fold(0, |acc, &x| acc << 8 | u32::from(x))
}

/// Undoes unsynchronisation: every `FF 00` loses its `00`.
fn resync(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut prev = 0u8;
    for &b in data {
        if !(prev == 0xFF && b == 0) {
            out.push(b);
        }
        prev = b;
    }
    out
}

fn parse_one(version: u8, flags: u8, body: &[u8], tags: &mut Tags) {
    let unsync = flags & 0x80 != 0;
    match version {
        2 => {
            // v2.2's bit 6 is compression, for which no scheme was ever defined.
            if flags & 0x40 != 0 {
                return;
            }
            let body = if unsync { resync(body) } else { body.to_vec() };
            frames(&body, 2, tags, |_, b| Some(b.to_vec()));
        }
        3 => {
            let body = if unsync { resync(body) } else { body.to_vec() };
            let mut body = &body[..];
            if flags & 0x40 != 0 {
                let Some(ext) = body.get(..4) else { return };
                let skip = (be(ext) as usize).saturating_add(4);
                body = body.get(skip..).unwrap_or_default();
            }
            frames(body, 3, tags, |fl, data| {
                // Compressed or encrypted frames are skipped; grouping adds a byte.
                if fl & 0x00C0 != 0 {
                    return None;
                }
                let skip = if fl & 0x0020 != 0 { 1 } else { 0 };
                data.get(skip..).map(<[u8]>::to_vec)
            });
        }
        4 => {
            let mut body = body;
            if flags & 0x40 != 0 {
                let Some(ext) = body.get(..4) else { return };
                body = body.get(syncsafe(ext) as usize..).unwrap_or_default();
            }
            frames(body, 4, tags, |fl, data| {
                if fl & 0x000C != 0 {
                    return None;
                }
                let mut skip = 0;
                if fl & 0x0040 != 0 {
                    skip += 1;
                }
                if fl & 0x0001 != 0 {
                    skip += 4;
                }
                let data = data.get(skip..)?;
                Some(if fl & 0x0002 != 0 || unsync {
                    resync(data)
                } else {
                    data.to_vec()
                })
            });
        }
        _ => {}
    }
}

/// Walks the frames of a tag body of ID3v2.`version`: ids of 3 bytes in v2.2, else 4, then
/// sizes (v2.4's syncsafe), then two flag bytes (not in v2.2). `data` turns a frame's flags and
/// raw bytes into its content, or `None` to skip it.
fn frames(body: &[u8], version: u8, tags: &mut Tags, data: impl Fn(u16, &[u8]) -> Option<Vec<u8>>) {
    let (id_len, header_len) = if version == 2 { (3, 6) } else { (4, 10) };
    let mut i = 0;
    let mut kept = 0;
    while i + header_len <= body.len() && kept < MAX_ENTRIES {
        let id = &body[i..i + id_len];
        if id[0] == 0 {
            break; // padding
        }
        let (size, flags) = match version {
            2 => (be(&body[i + 3..i + 6]) as usize, 0),
            3 => (
                be(&body[i + 4..i + 8]) as usize,
                be(&body[i + 8..i + 10]) as u16,
            ),
            _ => (v24_size(body, i), be(&body[i + 8..i + 10]) as u16),
        };
        let start = i + header_len;
        let Some(end) = start.checked_add(size).filter(|&e| e <= body.len()) else {
            break;
        };
        if let Some(key) = key_for(id)
            && let Some(content) = data(flags, &body[start..end])
        {
            kept += push_frame(key, &content, tags);
        }
        i = end;
    }
}

/// v2.4 frame sizes are syncsafe, but some writers (old iTunes) wrote them plainly. Where
/// the two readings differ, the plain one is taken only when it lands on the next frame (or
/// the end) and the syncsafe one does not.
fn v24_size(body: &[u8], i: usize) -> usize {
    let raw = &body[i + 4..i + 8];
    let safe = syncsafe(raw) as usize;
    let plain = be(raw) as usize;
    if raw.iter().all(|b| b & 0x80 == 0) && plain == safe {
        return safe;
    }
    let lands = |size: usize| {
        let next = i + 10 + size;
        next == body.len()
            || body.get(next..next + 4).is_some_and(|id| {
                id[0] == 0
                    || id
                        .iter()
                        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
            })
    };
    if !lands(safe) && lands(plain) {
        plain
    } else {
        safe
    }
}

/// The Vorbis-comment key a frame id maps to.
fn key_for(id: &[u8]) -> Option<&'static str> {
    Some(match id {
        b"TIT2" | b"TT2" => tag_keys::TITLE,
        b"TPE1" | b"TP1" => tag_keys::ARTIST,
        b"TPE2" | b"TP2" => "ALBUMARTIST",
        b"TALB" | b"TAL" => tag_keys::ALBUM,
        b"TDRC" | b"TYER" | b"TYE" => tag_keys::DATE,
        b"TRCK" | b"TRK" => tag_keys::TRACKNUMBER,
        b"TPOS" | b"TPA" => "DISCNUMBER",
        b"TCON" | b"TCO" => tag_keys::GENRE,
        b"TCOM" | b"TCM" => "COMPOSER",
        b"TCOP" | b"TCR" => tag_keys::COPYRIGHT,
        b"COMM" | b"COM" => tag_keys::COMMENT,
        _ => return None,
    })
}

/// Pushes a frame's values; returns how many.
fn push_frame(key: &'static str, content: &[u8], tags: &mut Tags) -> usize {
    let Some((&encoding, rest)) = content.split_first() else {
        return 0;
    };
    if key == tag_keys::COMMENT {
        // Language (3 bytes), a description, then the text.
        let Some(rest) = rest.get(3..) else { return 0 };
        let (description, text) = split_terminated(encoding, rest);
        // iTunes keeps machine data (normalisation, gapless figures) in described comments.
        if decode(encoding, description).starts_with("iTun") {
            return 0;
        }
        let text = decode(encoding, text);
        let text = text.trim_end_matches('\0');
        if text.is_empty() {
            return 0;
        }
        tags.push(key, text);
        return 1;
    }
    let text = decode(encoding, rest);
    let mut n = 0;
    // v2.4 separates several values with NUL.
    for value in text.split('\0').filter(|v| !v.is_empty()) {
        let value = if key == tag_keys::GENRE {
            genre(value)
        } else {
            value.to_string()
        };
        if !value.is_empty() {
            tags.push(key, value);
            n += 1;
        }
    }
    n
}

/// Splits at the first terminator of the encoding (one NUL, or two aligned for UTF-16).
fn split_terminated(encoding: u8, b: &[u8]) -> (&[u8], &[u8]) {
    if encoding == 1 || encoding == 2 {
        let mut i = 0;
        while i + 1 < b.len() {
            if b[i] == 0 && b[i + 1] == 0 {
                return (&b[..i], &b[i + 2..]);
            }
            i += 2;
        }
        (b, &[])
    } else {
        match b.iter().position(|&x| x == 0) {
            Some(i) => (&b[..i], &b[i + 1..]),
            None => (b, &[]),
        }
    }
}

/// Text in one of ID3v2's encodings: 0 ISO-8859-1, 1 UTF-16 with a byte-order mark,
/// 2 UTF-16BE, 3 UTF-8. Each UTF-16 value may carry its own BOM.
fn decode(encoding: u8, b: &[u8]) -> String {
    match encoding {
        1 | 2 => {
            let mut out = String::new();
            let mut units: Vec<u16> = Vec::with_capacity(b.len() / 2);
            let mut big = encoding == 2;
            let mut first = true;
            let flush = |units: &mut Vec<u16>, out: &mut String| {
                out.extend(char::decode_utf16(units.drain(..)).map(|c| c.unwrap_or('\u{FFFD}')));
            };
            for &[x0, x1] in b.as_chunks::<2>().0 {
                match (first, x0, x1) {
                    (_, 0, 0) => {
                        flush(&mut units, &mut out);
                        out.push('\0');
                        big = encoding == 2;
                        first = true;
                        continue;
                    }
                    (true, 0xFF, 0xFE) => big = false,
                    (true, 0xFE, 0xFF) => big = true,
                    (_, x, y) => units.push(if big {
                        u16::from_be_bytes([x, y])
                    } else {
                        u16::from_le_bytes([x, y])
                    }),
                }
                first = false;
            }
            flush(&mut units, &mut out);
            out
        }
        3 => String::from_utf8_lossy(b).into_owned(),
        _ => latin1(b),
    }
}

fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| char::from(c)).collect()
}

/// `TCON`: "(17)", "(17)Rock", "17", "(RX)" and "(CR)" become names; anything else stays.
fn genre(value: &str) -> String {
    let named = |n: &str| -> Option<String> {
        match n {
            "RX" => Some("Remix".into()),
            "CR" => Some("Cover".into()),
            _ => n
                .parse::<usize>()
                .ok()
                .and_then(|i| GENRES.get(i))
                .map(|g| (*g).to_string()),
        }
    };
    if let Some(rest) = value.strip_prefix('(')
        && let Some((n, after)) = rest.split_once(')')
    {
        if !after.is_empty() && !after.starts_with('(') {
            return after.to_string();
        }
        if let Some(name) = named(n) {
            return name;
        }
    }
    named(value).unwrap_or_else(|| value.to_string())
}

/// What a 128-byte ID3v1 (or v1.1) tag adds: only keys the ID3v2 tags did not give.
pub(super) fn parse_v1(tag: &[u8; 128], tags: &mut Tags) {
    if &tag[..3] != b"TAG" {
        return;
    }
    let field = |r: std::ops::Range<usize>| {
        let s = &tag[r];
        let s = &s[..s.iter().position(|&b| b == 0).unwrap_or(s.len())];
        latin1(s).trim_end().to_string()
    };
    let mut add = |key: &str, value: String| {
        if !value.is_empty() && tags.get(key).is_none() {
            tags.push(key, value);
        }
    };
    add(tag_keys::TITLE, field(3..33));
    add(tag_keys::ARTIST, field(33..63));
    add(tag_keys::ALBUM, field(63..93));
    add(tag_keys::DATE, field(93..97));
    // v1.1: a zero byte before the last comment byte makes that byte the track number.
    if tag[125] == 0 && tag[126] != 0 {
        add(tag_keys::COMMENT, field(97..125));
        add(tag_keys::TRACKNUMBER, tag[126].to_string());
    } else {
        add(tag_keys::COMMENT, field(97..127));
    }
    if let Some(g) = GENRES.get(usize::from(tag[127])) {
        add(tag_keys::GENRE, (*g).to_string());
    }
}

/// The length of an APEv2 tag whose 32-byte footer is `footer`, header included; `None` when
/// it is not one.
pub(super) fn ape_len(footer: &[u8; 32]) -> Option<u64> {
    if &footer[..8] != b"APETAGEX" {
        return None;
    }
    let size = u64::from(u32::from_le_bytes(footer[12..16].try_into().ok()?));
    let flags = u32::from_le_bytes(footer[20..24].try_into().ok()?);
    let header = if flags & 0x8000_0000 != 0 { 32 } else { 0 };
    Some(size + header)
}

/// ID3v1 genres 0-79 and Winamp's extension to 191.
static GENRES: [&str; 192] = [
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
    "Folk",
    "Folk-Rock",
    "National Folk",
    "Swing",
    "Fast Fusion",
    "Bebob",
    "Latin",
    "Revival",
    "Celtic",
    "Bluegrass",
    "Avantgarde",
    "Gothic Rock",
    "Progressive Rock",
    "Psychedelic Rock",
    "Symphonic Rock",
    "Slow Rock",
    "Big Band",
    "Chorus",
    "Easy Listening",
    "Acoustic",
    "Humour",
    "Speech",
    "Chanson",
    "Opera",
    "Chamber Music",
    "Sonata",
    "Symphony",
    "Booty Bass",
    "Primus",
    "Porn Groove",
    "Satire",
    "Slow Jam",
    "Club",
    "Tango",
    "Samba",
    "Folklore",
    "Ballad",
    "Power Ballad",
    "Rhythmic Soul",
    "Freestyle",
    "Duet",
    "Punk Rock",
    "Drum Solo",
    "A capella",
    "Euro-House",
    "Dance Hall",
    "Goa",
    "Drum & Bass",
    "Club-House",
    "Hardcore",
    "Terror",
    "Indie",
    "BritPop",
    "Negerpunk",
    "Polsk Punk",
    "Beat",
    "Christian Gangsta Rap",
    "Heavy Metal",
    "Black Metal",
    "Crossover",
    "Contemporary Christian",
    "Christian Rock",
    "Merengue",
    "Salsa",
    "Thrash Metal",
    "Anime",
    "JPop",
    "Synthpop",
    "Abstract",
    "Art Rock",
    "Baroque",
    "Bhangra",
    "Big Beat",
    "Breakbeat",
    "Chillout",
    "Downtempo",
    "Dub",
    "EBM",
    "Eclectic",
    "Electro",
    "Electroclash",
    "Emo",
    "Experimental",
    "Garage",
    "Global",
    "IDM",
    "Illbient",
    "Industro-Goth",
    "Jam Band",
    "Krautrock",
    "Leftfield",
    "Lounge",
    "Math Rock",
    "New Romantic",
    "Nu-Breakz",
    "Post-Punk",
    "Post-Rock",
    "Psytrance",
    "Shoegaze",
    "Space Rock",
    "Trop Rock",
    "World Music",
    "Neoclassical",
    "Audiobook",
    "Audio Theatre",
    "Neue Deutsche Welle",
    "Podcast",
    "Indie Rock",
    "G-Funk",
    "Dubstep",
    "Garage Rock",
    "Psybient",
];
