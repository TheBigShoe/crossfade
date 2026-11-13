//! Multistream decoding: a port of libopus `src/opus_multistream_decoder.c` and the layout
//! helpers of `opus_multistream.c` (3-clause BSD, see the crate's `COPYING`). A multistream
//! packet holds one Opus packet per stream, every one but the last self-delimited (RFC 6716
//! Appendix B); the channel mapping (RFC 7845 5.1.1) routes the decoded channels to the
//! output.

use crate::decoder::Decoder;
use crate::error::{Error, Result};
use crate::head::ChannelMapping;
use crate::packet::Packet;

/// Decodes the streams of one Ogg Opus stream (or any multistream source) into its output
/// channels at 48 kHz, interleaved `f32`.
#[derive(Debug, Clone)]
pub struct MultistreamDecoder {
    /// The first `coupled` stereo, the rest mono.
    streams: Vec<Decoder>,
    coupled: usize,
    /// Per output channel: the decoded channel it takes, 255 for silence.
    table: Vec<u8>,
    /// Each stream's output for the packet being decoded.
    bufs: Vec<Vec<f32>>,
}

impl MultistreamDecoder {
    /// A decoder for `mapping` giving `channels` output channels (`opus_multistream_decoder_
    /// create`): the table must have `channels` entries, each a decoded channel or 255, with
    /// 1 to 255 streams, at most as many coupled, and at most 255 decoded channels. Families
    /// 0, 1 and 255 (and the reserved ones, unpositioned as 255 is) decode; the ambisonic
    /// families 2 and 3 are refused.
    pub fn new(mapping: &ChannelMapping, channels: usize) -> Result<Self> {
        if matches!(mapping.family, 2 | 3) {
            return Err(Error::Unsupported(
                "ambisonic channel mapping (families 2 and 3) is not supported",
            ));
        }
        let streams = usize::from(mapping.streams);
        let coupled = usize::from(mapping.coupled);
        if !(1..=255).contains(&channels) || mapping.table.len() != channels {
            return Err(Error::BadArgument(
                "a channel mapping has one entry per output channel, 1 to 255",
            ));
        }
        if streams == 0 || coupled > streams || streams + coupled > 255 {
            return Err(Error::BadArgument(
                "a channel mapping has 1 to 255 streams, at most as many coupled",
            ));
        }
        if mapping
            .table
            .iter()
            .any(|&i| i != 255 && usize::from(i) >= streams + coupled)
        {
            return Err(Error::BadArgument(
                "a channel mapping names a channel not decoded",
            ));
        }
        let streams = (0..streams)
            .map(|s| Decoder::new(if s < coupled { 2 } else { 1 }))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            bufs: vec![Vec::new(); streams.len()],
            streams,
            coupled,
            table: mapping.table.clone(),
        })
    }

    /// Output channels.
    pub fn channels(&self) -> usize {
        self.table.len()
    }

    /// Each stream's final range after the last packet (`OPUS_GET_FINAL_RANGE` per stream).
    pub fn final_ranges(&self) -> Vec<u32> {
        self.streams.iter().map(Decoder::final_range).collect()
    }

    /// Sets every stream's output gain in Q8 dB (`OPUS_SET_GAIN`).
    pub fn set_gain(&mut self, gain: i16) {
        for d in &mut self.streams {
            d.set_gain(gain);
        }
    }

    /// Forgets the stream, as if new (`OPUS_RESET_STATE`); the gain is kept.
    pub fn reset(&mut self) {
        for d in &mut self.streams {
            d.reset();
        }
    }

    /// Decodes one multistream packet (`None`: lost, every stream concealed), appending its
    /// frames, interleaved, to `out`; returns how many frames it appended. An invalid packet
    /// (a stream missing or invalid, or streams of different durations) is an error that
    /// appends and changes nothing; any error appends nothing.
    pub fn decode(&mut self, packet: Option<&[u8]>, out: &mut Vec<f32>) -> Result<usize> {
        match packet {
            None => self.run(out, |d, _, buf| d.decode(None, buf)),
            Some(data) => {
                let packets = self.split(data)?;
                self.run(out, |d, s, buf| d.decode_packet(&packets[s], buf))
            }
        }
    }

    /// Conceals `frames` samples in every stream ([`Decoder::conceal`]).
    pub fn conceal(&mut self, frames: usize, out: &mut Vec<f32>) -> Result<usize> {
        self.run(out, |d, _, buf| d.conceal(frames, buf))
    }

    /// Every stream's forward error correction from `packet` in place of the lost packet
    /// before it ([`Decoder::decode_fec`]).
    pub fn decode_fec(&mut self, packet: &[u8], out: &mut Vec<f32>) -> Result<usize> {
        let packets = self.split(packet)?;
        self.run(out, |d, s, buf| d.decode_fec_packet(&packets[s], buf))
    }

    /// `opus_multistream_packet_validate`: the streams' packets, all of one duration.
    fn split<'a>(&self, mut data: &'a [u8]) -> Result<Vec<Packet<'a>>> {
        let n = self.streams.len();
        let mut packets = Vec::with_capacity(n);
        for s in 0..n {
            let p = if s + 1 < n {
                Packet::parse_self_delimited(data)?
            } else {
                Packet::parse(data)?
            };
            if packets
                .first()
                .is_some_and(|first: &Packet<'_>| first.duration() != p.duration())
            {
                return Err(Error::InvalidPacket(
                    "a multistream packet's streams differ in duration",
                ));
            }
            data = &data[p.byte_len()..];
            packets.push(p);
        }
        Ok(packets)
    }

    /// Decodes every stream with `f` into its buffer, then routes the channels to `out`.
    fn run(
        &mut self,
        out: &mut Vec<f32>,
        mut f: impl FnMut(&mut Decoder, usize, &mut Vec<f32>) -> Result<usize>,
    ) -> Result<usize> {
        let mut frames = None;
        for (s, (d, buf)) in self.streams.iter_mut().zip(&mut self.bufs).enumerate() {
            buf.clear();
            let n = f(d, s, buf)?;
            if *frames.get_or_insert(n) != n {
                return Err(Error::Corrupt(
                    "multistream streams decoded different lengths",
                ));
            }
        }
        let frames = frames.unwrap_or(0);
        let ch = self.table.len();
        let start = out.len();
        out.resize(start + frames * ch, 0.0);
        for (c, &m) in self.table.iter().enumerate() {
            let m = usize::from(m);
            let (buf, stride, at) = match m {
                255 => continue,
                m if m < 2 * self.coupled => (&self.bufs[m / 2], 2, m % 2),
                m => (&self.bufs[m - self.coupled], 1, 0),
            };
            for (o, &x) in out[start..]
                .iter_mut()
                .skip(c)
                .step_by(ch)
                .zip(buf.iter().skip(at).step_by(stride))
            {
                *o = x;
            }
        }
        Ok(frames)
    }
}

#[cfg(test)]
#[path = "multistream_tests.rs"]
mod tests;
