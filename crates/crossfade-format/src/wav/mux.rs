use crossfade_core::{Channel, CodecId, Error, MediaSink, Muxer, Packet, StreamInfo, Tags};
use std::io::SeekFrom;

const FORMAT_PCM: u16 = 1;
const FORMAT_FLOAT: u16 = 3;
const FORMAT_EXTENSIBLE: u16 = 0xFFFE;
/// Tail of the KSDATAFORMAT_SUBTYPE GUIDs, after the format tag.
const GUID_TAIL: [u8; 14] = [
    0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
];
/// WAVE channel-mask bit of each position, in the order the format requires.
const MASK_BITS: [(Channel, u32); 8] = [
    (Channel::FrontLeft, 0),
    (Channel::FrontRight, 1),
    (Channel::FrontCenter, 2),
    (Channel::Lfe, 3),
    (Channel::BackLeft, 4),
    (Channel::BackRight, 5),
    (Channel::SideLeft, 9),
    (Channel::SideRight, 10),
];

pub struct WavMuxer {
    sink: Box<dyn MediaSink>,
    bytes_per_frame: usize,
    data_len: u64,
    max_data_len: u64,
    header_len: u64,
    data_size_at: u64,
    fact_at: Option<u64>,
}

fn unsupported(what: &str) -> Error {
    Error::Unsupported {
        what: what.to_owned(),
    }
}

fn mask_for(positions: Option<&[Channel]>) -> u32 {
    let Some(positions) = positions else { return 0 };
    let mut last = None;
    let mut mask = 0;
    for p in positions {
        let Some(&(_, bit)) = MASK_BITS.iter().find(|(c, _)| c == p) else {
            return 0;
        };
        // A mask is a set in bit order; any other order would be lost.
        if last.is_some_and(|l| bit < l) {
            return 0;
        }
        last = Some(bit);
        mask |= 1 << bit;
    }
    mask
}

impl WavMuxer {
    pub fn new(sink: Box<dyn MediaSink>, stream: &StreamInfo) -> Result<Self, Error> {
        Self::with_tags(sink, stream, &Tags::new())
    }

    pub fn with_tags(
        mut sink: Box<dyn MediaSink>,
        stream: &StreamInfo,
        _tags: &Tags,
    ) -> Result<Self, Error> {
        let audio = &stream.audio;
        let format = audio.format();
        if stream.codec != CodecId::pcm_for(format) {
            return Err(Error::Malformed {
                what: "codec does not match the sample format".into(),
            });
        }
        let channels = audio.layout().channels();
        let rate = audio.sample_rate();
        let bytes_per_frame = audio.bytes_per_frame();
        let block_align = u16::try_from(bytes_per_frame)
            .map_err(|_| unsupported("frame too large for a WAV block align"))?;
        let byte_rate = u32::try_from(u64::from(rate) * u64::from(block_align))
            .map_err(|_| unsupported("byte rate does not fit 32 bits"))?;
        let bits = u16::try_from(format.bits()).map_err(|_| Error::Overflow)?;
        let float = format.is_float();
        let plain = !float && bits <= 16 && channels <= 2 && rate <= 48000;

        let mut h = Vec::with_capacity(80);
        h.extend_from_slice(b"RIFF\0\0\0\0WAVEfmt ");
        let tag = if float { FORMAT_FLOAT } else { FORMAT_PCM };
        let fmt_len: u32 = if plain {
            16
        } else if float {
            18
        } else {
            40
        };
        h.extend_from_slice(&fmt_len.to_le_bytes());
        h.extend_from_slice(
            &(if plain || float {
                tag
            } else {
                FORMAT_EXTENSIBLE
            })
            .to_le_bytes(),
        );
        h.extend_from_slice(&channels.to_le_bytes());
        h.extend_from_slice(&rate.to_le_bytes());
        h.extend_from_slice(&byte_rate.to_le_bytes());
        h.extend_from_slice(&block_align.to_le_bytes());
        h.extend_from_slice(&bits.to_le_bytes());
        if float {
            h.extend_from_slice(&0u16.to_le_bytes());
        } else if !plain {
            h.extend_from_slice(&22u16.to_le_bytes());
            h.extend_from_slice(&bits.to_le_bytes());
            h.extend_from_slice(&mask_for(audio.layout().positions()).to_le_bytes());
            h.extend_from_slice(&tag.to_le_bytes());
            h.extend_from_slice(&GUID_TAIL);
        }
        let mut fact_at = None;
        if float {
            h.extend_from_slice(b"fact\x04\0\0\0");
            fact_at = Some(h.len() as u64);
            h.extend_from_slice(&[0; 4]);
        }
        h.extend_from_slice(b"data\0\0\0\0");
        let header_len = h.len() as u64;
        sink.write_all(&h)?;
        Ok(Self {
            sink,
            bytes_per_frame,
            data_len: 0,
            // RIFF size covers all but the first 8 bytes, plus a pad byte;
            // 0xFFFFFFFF in `data` means "streaming" so stay below it.
            max_data_len: u64::from(u32::MAX) - (header_len - 8) - 1,
            header_len,
            data_size_at: header_len - 4,
            fact_at,
        })
    }

    fn patch(&mut self, at: u64, value: u32) -> Result<(), Error> {
        self.sink.seek(SeekFrom::Start(at))?;
        self.sink.write_all(&value.to_le_bytes())?;
        Ok(())
    }
}

impl Muxer for WavMuxer {
    fn write_packet(&mut self, packet: &Packet) -> Result<(), Error> {
        let len = packet.data.len();
        if !len.is_multiple_of(self.bytes_per_frame) {
            return Err(Error::Malformed {
                what: "packet is not a whole number of frames".into(),
            });
        }
        let total = self.data_len.saturating_add(len as u64);
        if total > self.max_data_len {
            return Err(unsupported("RF64 not implemented"));
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
        // Bounded by `max_data_len`, so these fit 32 bits.
        let data = u32::try_from(self.data_len).map_err(|_| Error::Overflow)?;
        let riff = u32::try_from(self.header_len - 8 + self.data_len + pad)
            .map_err(|_| Error::Overflow)?;
        self.patch(4, riff)?;
        self.patch(self.data_size_at, data)?;
        if let Some(at) = self.fact_at {
            let frames = self.data_len / self.bytes_per_frame as u64;
            self.patch(at, u32::try_from(frames).map_err(|_| Error::Overflow)?)?;
        }
        self.sink.finish()?;
        Ok(())
    }
}
