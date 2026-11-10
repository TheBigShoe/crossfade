use super::{TEXT_CAP, extended};
use crossfade_core::{CodecId, Error, MediaSink, Muxer, Packet, StreamInfo, Tags, tag_keys};
use std::io::SeekFrom;

const FVER_AIFC_1: u32 = 0xA280_5140;
/// Chunk sizes are signed 32-bit: the `SSND` chunk (data plus its 8-byte header) must fit.
const MAX_DATA_LEN: u64 = i32::MAX as u64 - 8;

pub struct AiffMuxer {
    sink: Box<dyn MediaSink>,
    bytes_per_frame: usize,
    data_len: u64,
    header_len: u64,
    comm_frames_at: u64,
    ssnd_size_at: u64,
}

fn unsupported(what: String) -> Error {
    Error::Unsupported { what }
}

/// AIFF-C compression type and name.
type Compression = (&'static [u8; 4], &'static str);

/// The compression (`None`: plain AIFF) and bits per sample.
fn describe(codec: CodecId) -> Result<(Option<Compression>, u16), Error> {
    Ok(match codec {
        CodecId::PcmS8 => (None, 8),
        CodecId::PcmS16Be => (None, 16),
        CodecId::PcmS24Be => (None, 24),
        CodecId::PcmS32Be => (None, 32),
        CodecId::PcmF32Be => (Some((b"fl32", "32-bit floating point")), 32),
        CodecId::PcmF64Be => (Some((b"fl64", "64-bit floating point")), 64),
        other => return Err(unsupported(format!("aiff cannot hold {}", other.name()))),
    })
}

/// Text chunks for the tags AIFF has a place for: the first value of each single-instance
/// key, every comment. Values the reader would skip as oversized are left out.
fn text_chunks(tags: &Tags) -> Vec<u8> {
    let mut out = Vec::new();
    let mut put = |id: &[u8; 4], text: &str| {
        let bytes = text.as_bytes();
        if bytes.len() as u64 > TEXT_CAP {
            return;
        }
        out.extend_from_slice(id);
        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        out.extend_from_slice(bytes);
        if bytes.len() % 2 == 1 {
            out.push(0);
        }
    };
    for (key, id) in [
        (tag_keys::TITLE, b"NAME"),
        (tag_keys::ARTIST, b"AUTH"),
        (tag_keys::COPYRIGHT, b"(c) "),
    ] {
        if let Some(v) = tags.get(key) {
            put(id, v);
        }
    }
    for t in tags
        .iter()
        .filter(|t| t.key.eq_ignore_ascii_case(tag_keys::COMMENT))
    {
        put(b"ANNO", &t.value);
    }
    out
}

impl AiffMuxer {
    pub fn new(
        mut sink: Box<dyn MediaSink>,
        stream: &StreamInfo,
        tags: &Tags,
    ) -> Result<Self, Error> {
        let (aifc, bits) = describe(stream.codec)?;
        let audio = &stream.audio;
        if stream.codec.pcm_sample_format() != Some(audio.format()) {
            return Err(Error::Malformed {
                what: "codec does not match the sample format".into(),
            });
        }
        let channels = i16::try_from(audio.layout().channels())
            .map_err(|_| unsupported("more than 32767 channels".into()))?;

        let mut h = Vec::with_capacity(96);
        h.extend_from_slice(b"FORM\0\0\0\0");
        h.extend_from_slice(if aifc.is_some() { b"AIFC" } else { b"AIFF" });
        if aifc.is_some() {
            h.extend_from_slice(b"FVER\0\0\0\x04");
            h.extend_from_slice(&FVER_AIFC_1.to_be_bytes());
        }
        h.extend_from_slice(b"COMM");
        // Pascal string: count byte and text, padded to an even total.
        let name_len = aifc.map_or(0, |(_, name)| 1 + name.len() + (1 + name.len()) % 2);
        let comm_len = 18 + aifc.map_or(0, |_| 4 + name_len) as u32;
        h.extend_from_slice(&comm_len.to_be_bytes());
        h.extend_from_slice(&channels.to_be_bytes());
        let comm_frames_at = h.len() as u64;
        h.extend_from_slice(&[0; 4]);
        h.extend_from_slice(&bits.to_be_bytes());
        h.extend_from_slice(&extended::encode(audio.sample_rate()));
        if let Some((kind, name)) = aifc {
            h.extend_from_slice(kind);
            h.push(name.len() as u8);
            h.extend_from_slice(name.as_bytes());
            if (1 + name.len()) % 2 == 1 {
                h.push(0);
            }
        }
        h.extend_from_slice(&text_chunks(tags));
        h.extend_from_slice(b"SSND\0\0\0\0\0\0\0\0\0\0\0\0");
        let header_len = h.len() as u64;
        sink.write_all(&h)?;
        Ok(Self {
            sink,
            bytes_per_frame: audio.bytes_per_frame(),
            data_len: 0,
            header_len,
            comm_frames_at,
            ssnd_size_at: header_len - 12,
        })
    }

    fn patch(&mut self, at: u64, value: u32) -> Result<(), Error> {
        self.sink.seek(SeekFrom::Start(at))?;
        self.sink.write_all(&value.to_be_bytes())?;
        Ok(())
    }
}

impl Muxer for AiffMuxer {
    fn write_packet(&mut self, packet: &Packet) -> Result<(), Error> {
        let len = packet.data.len();
        if !len.is_multiple_of(self.bytes_per_frame) {
            return Err(Error::Malformed {
                what: "packet is not a whole number of frames".into(),
            });
        }
        let total = self.data_len.saturating_add(len as u64);
        if total > MAX_DATA_LEN {
            return Err(unsupported("AIFF sound data over 2 GiB".into()));
        }
        self.sink.write_all(&packet.data)?;
        self.data_len = total;
        Ok(())
    }

    fn finish(&mut self) -> Result<(), Error> {
        let pad = self.data_len % 2;
        if pad == 1 {
            self.sink.write_all(&[0])?;
        }
        let frames = self.data_len / self.bytes_per_frame as u64;
        // Bounded by MAX_DATA_LEN and the header, so these fit 32 bits.
        let form = u32::try_from(self.header_len - 8 + self.data_len + pad)
            .map_err(|_| Error::Overflow)?;
        let ssnd = u32::try_from(8 + self.data_len).map_err(|_| Error::Overflow)?;
        let frames = u32::try_from(frames).map_err(|_| Error::Overflow)?;
        self.patch(4, form)?;
        self.patch(self.comm_frames_at, frames)?;
        self.patch(self.ssnd_size_at, ssnd)?;
        self.sink.finish()?;
        Ok(())
    }
}
