//! FLAC decoder (RFC 9639): one packet, one whole frame, in; one block of interleaved samples
//! out, left-justified in the stream's container format. The hash of the raw samples is
//! checked against STREAMINFO once the stream is drained.

use super::crc::crc16;
use super::frame::{self, ChannelAssignment, FrameHeader, HeaderError};
use super::streaminfo::StreamInfoBlock;
use super::subframe::{Bits, read_subframe};
use crate::md5::Md5;
use crossfade_core::{
    AudioFrame, ChannelLayout, Decoder, Error, Packet, SampleFormat, Samples, StreamInfo,
};

fn malformed(what: &str) -> Error {
    Error::Malformed {
        what: format!("flac: {what}"),
    }
}

fn format_for(bits: u8) -> SampleFormat {
    match bits {
        0..=8 => SampleFormat::U8,
        9..=16 => SampleFormat::S16,
        17..=24 => SampleFormat::S24,
        _ => SampleFormat::S32,
    }
}

struct FlacDecoder {
    info: StreamInfoBlock,
    format: SampleFormat,
    layout: ChannelLayout,
    /// One buffer per channel, reused from frame to frame.
    chans: Vec<Vec<i64>>,
    md5: Md5,
    md5_bytes: Vec<u8>,
    /// The decoded block; `receive_frame` trades it for the caller's frame, so the two
    /// buffers keep being reused.
    frame: AudioFrame,
    ready: bool,
    decoded: u64,
    draining: bool,
    end_checked: bool,
    /// A frame failed: the totals and the hash no longer say anything about the stream.
    failed: bool,
}

pub fn decoder(stream: &StreamInfo) -> Result<Box<dyn Decoder>, Error> {
    let info = StreamInfoBlock::parse(&stream.extradata)?;
    let format = format_for(info.bits_per_sample);
    if stream.audio.format() != format
        || stream.audio.layout().channels() != u16::from(info.channels)
        || stream.audio.sample_rate() != info.sample_rate
    {
        return Err(malformed(
            "stream description disagrees with its STREAMINFO",
        ));
    }
    Ok(Box::new(FlacDecoder {
        info,
        format,
        layout: stream.audio.layout().clone(),
        chans: vec![Vec::new(); usize::from(info.channels)],
        md5: Md5::new(),
        md5_bytes: Vec::new(),
        frame: AudioFrame::new(&stream.audio),
        ready: false,
        decoded: 0,
        draining: false,
        end_checked: false,
        failed: false,
    }))
}

/// Writes `chans` interleaved into `out` through `f`.
fn interleave<T: Copy + Default>(
    chans: &[Vec<i64>],
    block: usize,
    out: &mut Vec<T>,
    f: impl Fn(i64) -> T,
) {
    let nch = chans.len();
    out.clear();
    out.resize(block * nch, T::default());
    for (c, ch) in chans.iter().enumerate() {
        for (dst, &v) in out[c..].iter_mut().step_by(nch).zip(ch) {
            *dst = f(v);
        }
    }
}

impl FlacDecoder {
    fn check_parameters(&self, h: &FrameHeader) -> Result<(), Error> {
        let unsupported = |what: &str, was: u32, now: u32| Error::Unsupported {
            what: format!("flac: {what} changed from {was} to {now} mid-stream"),
        };
        let channels = h.channels.channels();
        if channels != self.info.channels {
            return Err(unsupported(
                "channel count",
                u32::from(self.info.channels),
                u32::from(channels),
            ));
        }
        let bits = h.bits_per_sample.unwrap_or(self.info.bits_per_sample);
        if bits != self.info.bits_per_sample {
            return Err(unsupported(
                "bit depth",
                u32::from(self.info.bits_per_sample),
                u32::from(bits),
            ));
        }
        let rate = h.sample_rate.unwrap_or(self.info.sample_rate);
        if rate != self.info.sample_rate {
            return Err(unsupported("sample rate", self.info.sample_rate, rate));
        }
        Ok(())
    }

    fn decode_frame(&mut self, packet: &Packet) -> Result<(), Error> {
        let data = &packet.data[..];
        let h = frame::parse_header(data).map_err(|e| match e {
            HeaderError::NeedMoreData => malformed("frame header cut short"),
            HeaderError::Invalid(why) => malformed(why),
        })?;
        self.check_parameters(&h)?;
        if data.len() < h.header_len + 2 || crc16(data) != 0 {
            return Err(malformed("frame CRC-16 mismatch"));
        }
        let block = h.block_size as usize;
        let bps = u32::from(self.info.bits_per_sample);
        let mut r = Bits::new(&data[h.header_len..data.len() - 2]);
        // The side channel of a stereo pair has one more bit.
        let side = match h.channels {
            ChannelAssignment::LeftSide | ChannelAssignment::MidSide => Some(1),
            ChannelAssignment::SideRight => Some(0),
            ChannelAssignment::Independent(_) => None,
        };
        for (c, out) in self.chans.iter_mut().enumerate() {
            read_subframe(&mut r, out, block, bps + u32::from(side == Some(c)))?;
        }
        r.align();
        // The CRC-16 follows the subframes. Zero bytes after it are padding the container
        // reader could not tell from the frame (the CRC-16 stays zero over zeros).
        let end = data.len() - 2 - (r.bits_left() / 8) as usize;
        if crc16(&data[..end + 2]) != 0 {
            return Err(malformed("frame CRC-16 mismatch"));
        }
        if data[end + 2..].iter().any(|&b| b != 0) {
            return Err(malformed(
                "unused bytes between the subframes and the CRC-16",
            ));
        }
        self.decorrelate(h.channels);
        self.emit(block, packet.pts);
        self.decoded += block as u64;
        Ok(())
    }

    fn decorrelate(&mut self, assignment: ChannelAssignment) {
        if matches!(assignment, ChannelAssignment::Independent(_)) {
            return;
        }
        // The other assignments are stereo pairs, so two buffers exist.
        let (first, rest) = self.chans.split_at_mut(1);
        let (a, b) = (&mut first[0], &mut rest[0]);
        match assignment {
            ChannelAssignment::Independent(_) => {}
            ChannelAssignment::LeftSide => {
                for (l, s) in a.iter().zip(b.iter_mut()) {
                    *s = l.wrapping_sub(*s);
                }
            }
            ChannelAssignment::SideRight => {
                for (s, r) in a.iter_mut().zip(b.iter()) {
                    *s = s.wrapping_add(*r);
                }
            }
            ChannelAssignment::MidSide => {
                for (m, s) in a.iter_mut().zip(b.iter_mut()) {
                    // The mid channel dropped the low bit of L + R; the side's parity restores it.
                    let sum = (m.wrapping_shl(1)) | (*s & 1);
                    *m = sum.wrapping_add(*s) >> 1;
                    *s = sum.wrapping_sub(*s) >> 1;
                }
            }
        }
    }

    /// Hashes the raw samples and fills `self.frame` in the container format.
    fn emit(&mut self, block: usize, pts: i64) {
        let nch = self.chans.len();
        let bits = u32::from(self.info.bits_per_sample);
        let width = bits.div_ceil(8) as usize;
        self.md5_bytes.clear();
        self.md5_bytes.resize(block * nch * width, 0);
        for (c, ch) in self.chans.iter().enumerate() {
            let slots = self.md5_bytes.chunks_exact_mut(width).skip(c).step_by(nch);
            for (slot, &v) in slots.zip(ch) {
                slot.copy_from_slice(&v.to_le_bytes()[..width]);
            }
        }
        self.md5.update(&self.md5_bytes);

        let f = &mut self.frame;
        f.pts = pts;
        f.sample_rate = self.info.sample_rate;
        if f.layout != self.layout {
            f.layout = self.layout.clone();
        }
        if f.samples.format() != self.format {
            f.samples = Samples::new(self.format);
        }
        match &mut f.samples {
            Samples::U8(out) => {
                let up = 8 - bits;
                interleave(&self.chans, block, out, |v| ((v << up) + 128) as u8);
            }
            Samples::S16(out) => {
                let up = 16 - bits;
                interleave(&self.chans, block, out, |v| (v << up) as i16);
            }
            Samples::S24(out) => {
                let up = 24 - bits;
                interleave(&self.chans, block, out, |v| (v << up) as i32);
            }
            Samples::S32(out) => {
                let up = 32 - bits;
                interleave(&self.chans, block, out, |v| (v << up) as i32);
            }
            Samples::F32(_) | Samples::F64(_) => unreachable!("format_for never picks floats"),
        }
        self.ready = true;
    }

    fn end_of_stream(&mut self) -> Result<(), Error> {
        if self.failed {
            return Ok(());
        }
        let total = self.info.total_samples;
        if total != 0 && total != self.decoded {
            return Err(Error::Malformed {
                what: format!(
                    "flac: stream ended after {} of {total} samples",
                    self.decoded
                ),
            });
        }
        if self.info.md5 != [0; 16] && std::mem::take(&mut self.md5).finalize() != self.info.md5 {
            return Err(malformed(
                "decoded audio does not match the MD5 in the stream header",
            ));
        }
        Ok(())
    }
}

impl Decoder for FlacDecoder {
    fn send_packet(&mut self, packet: &Packet) -> Result<(), Error> {
        self.decode_frame(packet)
            .inspect_err(|_| self.failed = true)
    }

    fn receive_frame(&mut self, out: &mut AudioFrame) -> Result<bool, Error> {
        if self.ready {
            self.ready = false;
            std::mem::swap(out, &mut self.frame);
            return Ok(true);
        }
        if self.draining && !self.end_checked {
            self.end_checked = true;
            self.end_of_stream()?;
        }
        Ok(false)
    }

    fn drain(&mut self) {
        self.draining = true;
    }
}

#[cfg(test)]
#[path = "decode_tests.rs"]
mod tests;
