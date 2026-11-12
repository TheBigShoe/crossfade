//! The sample description (`stsd`): which codec, and what its header says about the audio.

use super::asc;
use super::boxes::{Children, Cursor, FourCc, find, malformed, name};
use crossfade_core::{ChannelLayout, CodecId, Error, SampleFormat};

/// What the track's one sample description says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Entry {
    pub(super) codec: CodecId,
    /// The codec's header: ALAC's 24-byte ALACSpecificConfig, AAC's AudioSpecificConfig as
    /// stored; empty for MP3.
    pub(super) extradata: Vec<u8>,
    /// 0 when the description does not know (the track's timescale stands in).
    pub(super) sample_rate: u32,
    pub(super) layout: ChannelLayout,
    pub(super) format: SampleFormat,
    pub(super) bits_per_raw_sample: Option<u8>,
}

fn encrypted() -> Error {
    Error::Unsupported {
        what: "mp4: encrypted audio is not supported".into(),
    }
}

/// The fixed fields of an audio sample entry, and where its child boxes start.
struct Sound<'a> {
    channels: u16,
    sample_rate: u32,
    children: &'a [u8],
}

fn sound(body: &[u8]) -> Result<Sound<'_>, Error> {
    let mut c = Cursor::new(body, "audio sample entry");
    c.skip(8)?; // reserved, data reference index
    let version = c.u16()?;
    c.skip(6)?; // revision, vendor
    let mut channels = c.u16()?;
    c.skip(6)?; // sample size, compression id, packet size
    let mut sample_rate = c.u32()? >> 16;
    match version {
        0 => {}
        // QuickTime's version 1 adds four 32-bit fields; version 2 replaces the rate and
        // channels with wider ones.
        1 => c.skip(16)?,
        2 => {
            c.skip(4)?;
            let rate = f64::from_bits(c.u64()?);
            channels = u16::try_from(c.u32()?).unwrap_or(0);
            c.skip(20)?;
            sample_rate = if rate.is_finite() && (1.0..=4_294_967_295.0).contains(&rate) {
                rate as u32
            } else {
                0
            };
        }
        v => {
            return Err(malformed(format!(
                "audio sample entry version {v} is unknown"
            )));
        }
    }
    Ok(Sound {
        channels,
        sample_rate,
        children: c.rest(),
    })
}

/// The children to look in: the entry's own, or those of a QuickTime `wave` box among them.
fn inner<'a>(children: &'a [u8], kind: &FourCc) -> Result<Option<&'a [u8]>, Error> {
    for child in Children::new(children) {
        let (k, body) = child?;
        if &k == b"sinf" {
            return Err(encrypted());
        }
        if &k == kind {
            return Ok(Some(body));
        }
    }
    match find(children, b"wave")? {
        Some(wave) => inner(wave, kind),
        None => Ok(None),
    }
}

fn alac(s: &Sound<'_>) -> Result<Entry, Error> {
    let body = inner(s.children, b"alac")?.ok_or_else(|| malformed("alac entry has no config"))?;
    // A full box: version and flags, then the 24 bytes.
    let config = body
        .get(4..4 + crossfade_alac::Config::LEN)
        .ok_or_else(|| malformed("alac config is cut off"))?;
    let parsed = crossfade_alac::Config::parse(config).map_err(|e| Error::Unsupported {
        what: e.to_string(),
    })?;
    let wave = crossfade_alac::wave_layout(parsed.channels)
        .ok_or_else(|| malformed("alac channel count"))?;
    let layout = ChannelLayout::from_mask(wave.mask, u16::from(parsed.channels))
        .ok_or_else(|| malformed("alac channel layout"))?;
    let (format, raw) = match parsed.bit_depth {
        16 => (SampleFormat::S16, None),
        20 => (SampleFormat::S24, Some(20)),
        24 => (SampleFormat::S24, None),
        _ => (SampleFormat::S32, None),
    };
    Ok(Entry {
        codec: CodecId::Alac,
        extradata: config.to_vec(),
        sample_rate: if parsed.sample_rate != 0 {
            parsed.sample_rate
        } else {
            s.sample_rate
        },
        layout,
        format,
        bits_per_raw_sample: raw,
    })
}

/// A descriptor's tag and body (14496-1, 8.3.3: the length is 1 to 4 bytes of 7 bits).
fn descriptor<'a>(c: &mut Cursor<'a>) -> Result<(u8, &'a [u8]), Error> {
    let tag = c.u8()?;
    let mut len = 0usize;
    for _ in 0..4 {
        let b = c.u8()?;
        len = len << 7 | usize::from(b & 0x7F);
        if b & 0x80 == 0 {
            break;
        }
    }
    Ok((tag, c.bytes(len)?))
}

/// The object type and the decoder specific info (the AudioSpecificConfig for AAC).
fn esds(body: &[u8]) -> Result<(u8, Option<&[u8]>), Error> {
    let mut c = Cursor::new(body, "esds");
    c.version()?;
    let (tag, es) = descriptor(&mut c)?;
    if tag != 0x03 {
        return Err(malformed("esds holds no ES descriptor"));
    }
    let mut c = Cursor::new(es, "esds");
    c.skip(2)?; // ES_ID
    let flags = c.u8()?;
    if flags & 0x80 != 0 {
        c.skip(2)?;
    }
    if flags & 0x40 != 0 {
        let len = c.u8()?;
        c.skip(usize::from(len))?;
    }
    if flags & 0x20 != 0 {
        c.skip(2)?;
    }
    while c.remaining() > 0 {
        let (tag, body) = descriptor(&mut c)?;
        if tag != 0x04 {
            continue;
        }
        let mut d = Cursor::new(body, "esds");
        let object_type = d.u8()?;
        d.skip(12)?; // stream type, buffer size, bit rates
        while d.remaining() > 0 {
            let (tag, info) = descriptor(&mut d)?;
            if tag == 0x05 {
                return Ok((object_type, Some(info)));
            }
        }
        return Ok((object_type, None));
    }
    Err(malformed("esds holds no decoder config"))
}

fn plain_layout(channels: u16) -> Result<ChannelLayout, Error> {
    if channels == 0 {
        return Err(malformed("audio sample entry has no channels"));
    }
    ChannelLayout::default_for(channels)
}

fn mp4a(s: &Sound<'_>) -> Result<Entry, Error> {
    let body = inner(s.children, b"esds")?.ok_or_else(|| malformed("mp4a entry has no esds"))?;
    let (object_type, info) = esds(body)?;
    let lossy = |codec, extradata: Vec<u8>, sample_rate, layout| Entry {
        codec,
        extradata,
        sample_rate,
        layout,
        format: SampleFormat::F32,
        bits_per_raw_sample: None,
    };
    match object_type {
        // MPEG-4 audio, and MPEG-2 AAC main, LC and SSR.
        0x40 | 0x66..=0x68 => {
            let Some(info) = info else {
                if object_type == 0x40 {
                    return Err(malformed("aac in mp4a has no AudioSpecificConfig"));
                }
                return Ok(lossy(
                    CodecId::Aac,
                    Vec::new(),
                    s.sample_rate,
                    plain_layout(s.channels)?,
                ));
            };
            let a = asc::parse(info)?;
            Ok(lossy(CodecId::Aac, info.to_vec(), a.sample_rate, a.layout))
        }
        // MPEG-2 and MPEG-1 audio (layer III, and the others the same decoder takes).
        0x69 | 0x6B => Ok(lossy(
            CodecId::Mp3,
            Vec::new(),
            s.sample_rate,
            plain_layout(s.channels)?,
        )),
        t => Err(Error::Unsupported {
            what: format!("mp4: mp4a with object type 0x{t:02X} is not supported"),
        }),
    }
}

/// The track's sample description: exactly one entry is read (`stsc` refuses tracks that use
/// another).
pub(super) fn parse(stsd: &[u8]) -> Result<Entry, Error> {
    let mut c = Cursor::new(stsd, "stsd");
    c.version()?;
    let n = c.u32()?;
    if n == 0 {
        return Err(malformed("stsd has no sample description"));
    }
    let (kind, body) = Children::new(c.rest())
        .next()
        .ok_or_else(|| malformed("stsd is empty"))??;
    match &kind {
        b"alac" => alac(&sound(body)?),
        b"mp4a" => mp4a(&sound(body)?),
        b".mp3" => {
            let s = sound(body)?;
            Ok(Entry {
                codec: CodecId::Mp3,
                extradata: Vec::new(),
                sample_rate: s.sample_rate,
                layout: plain_layout(s.channels)?,
                format: SampleFormat::F32,
                bits_per_raw_sample: None,
            })
        }
        b"enca" => Err(encrypted()),
        other => Err(Error::Unsupported {
            what: format!("mp4: the '{}' codec is not supported", name(*other)),
        }),
    }
}
