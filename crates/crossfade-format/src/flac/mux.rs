//! FLAC writer: `fLaC`, STREAMINFO, one VORBIS_COMMENT block, then the frames as given.
//!
//! The STREAMINFO written first is provisional when the encoder produced it (totals and MD5
//! unknown); `set_extradata` brings the final one and `finish` seeks back to overwrite it. On a
//! sink that cannot seek the provisional header stays, which is a valid stream with unknown
//! totals. Stream copy hands over the source's STREAMINFO and never calls `set_extradata`, so
//! nothing is patched.

use crate::vorbis_comment;
use crossfade_core::{CodecId, Error, MediaSink, Muxer, Packet, SampleFormat, StreamInfo, Tags};
use crossfade_flac::streaminfo::StreamInfoBlock;
use std::io::SeekFrom;

const MARKER: &[u8; 4] = b"fLaC";
const TYPE_VORBIS_COMMENT: u8 = 4;
const LAST_BLOCK: u8 = 0x80;
/// The marker plus the STREAMINFO block header: where its body starts.
const STREAMINFO_AT: u64 = 8;
/// Block lengths are 24 bits.
const MAX_BLOCK_LEN: usize = (1 << 24) - 1;

struct FlacMuxer {
    sink: Box<dyn MediaSink>,
    final_header: Option<[u8; StreamInfoBlock::LEN]>,
}

fn synthesised(stream: &StreamInfo) -> Result<[u8; StreamInfoBlock::LEN], Error> {
    let audio = &stream.audio;
    let channels = audio.layout().channels();
    if channels > 8 {
        return Err(Error::Unsupported {
            what: format!("flac holds at most 8 channels, got {channels}"),
        });
    }
    if audio.sample_rate() >= 1 << 20 {
        return Err(Error::Unsupported {
            what: format!(
                "flac sample rates stop at 1048575 Hz, got {}",
                audio.sample_rate()
            ),
        });
    }
    let bits = stream.bits_per_raw_sample.unwrap_or(match audio.format() {
        SampleFormat::U8 => 8,
        SampleFormat::S16 => 16,
        SampleFormat::S24 => 24,
        SampleFormat::S32 | SampleFormat::F32 | SampleFormat::F64 => 32,
    });
    Ok(StreamInfoBlock {
        // Unknown until the encoder's final header arrives.
        min_block_size: 0,
        max_block_size: 0,
        min_frame_size: 0,
        max_frame_size: 0,
        sample_rate: audio.sample_rate(),
        channels: channels as u8,
        bits_per_sample: bits,
        total_samples: stream.frames.filter(|&n| n < 1 << 36).unwrap_or(0),
        md5: [0; 16],
    }
    .write())
}

fn checked(extradata: &[u8]) -> Result<[u8; StreamInfoBlock::LEN], Error> {
    StreamInfoBlock::parse(extradata).map_err(super::engine_error)?;
    Ok(extradata.try_into().expect("parse accepted 34 bytes"))
}

fn block_header(flags_and_type: u8, len: usize) -> [u8; 4] {
    let l = (len as u32).to_be_bytes();
    [flags_and_type, l[1], l[2], l[3]]
}

pub fn create(
    mut sink: Box<dyn MediaSink>,
    stream: &StreamInfo,
    tags: &Tags,
) -> Result<Box<dyn Muxer>, Error> {
    if stream.codec != CodecId::Flac {
        return Err(Error::Unsupported {
            what: format!("flac files cannot hold {}", stream.codec.name()),
        });
    }
    let info = if stream.extradata.len() == StreamInfoBlock::LEN {
        checked(&stream.extradata)?
    } else {
        synthesised(stream)?
    };
    let mut comment = Vec::new();
    vorbis_comment::write(
        &format!("Crossfade {}", env!("CARGO_PKG_VERSION")),
        tags,
        &mut comment,
    );
    if comment.len() > MAX_BLOCK_LEN {
        return Err(Error::Unsupported {
            what: "tags are too large for one flac metadata block".into(),
        });
    }
    let mut head = Vec::with_capacity(46 + comment.len());
    head.extend(MARKER);
    head.extend(block_header(0, StreamInfoBlock::LEN));
    head.extend(info);
    head.extend(block_header(
        LAST_BLOCK | TYPE_VORBIS_COMMENT,
        comment.len(),
    ));
    head.extend(comment);
    sink.write_all(&head)?;
    Ok(Box::new(FlacMuxer {
        sink,
        final_header: None,
    }))
}

impl Muxer for FlacMuxer {
    fn write_packet(&mut self, packet: &Packet) -> Result<(), Error> {
        self.sink.write_all(&packet.data)?;
        Ok(())
    }

    fn set_extradata(&mut self, extradata: &[u8]) -> Result<(), Error> {
        self.final_header = Some(checked(extradata)?);
        Ok(())
    }

    fn finish(&mut self) -> Result<(), Error> {
        if let Some(header) = self.final_header {
            // A sink that cannot seek keeps the provisional header, which is still valid.
            if self.sink.seek(SeekFrom::Start(STREAMINFO_AT)).is_ok() {
                self.sink.write_all(&header)?;
                self.sink.seek(SeekFrom::End(0))?;
            }
        }
        self.sink.finish()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
