//! Multistream encoding for mapping family 1 (RFC 7845 5.1.1.2): the channels of a Vorbis
//! layout as stereo and mono streams, each with its own [`Encoder`], their packets joined
//! with every one but the last self-delimited (RFC 6716 Appendix B). The layouts are
//! libopus's `vorbis_mappings`; there is no surround masking or LFE mode yet, the bitrate is
//! shared by channel count.

use crate::encoder::Encoder;
use crate::error::{Error, Result};
use crate::head::{ChannelMapping, OpusHead};

/// Per channel count: streams, coupled streams, and for each output channel the decoded
/// channel it takes.
const VORBIS_MAPPINGS: [(u8, u8, &[u8]); 8] = [
    (1, 0, &[0]),
    (1, 1, &[0, 1]),
    (2, 1, &[0, 2, 1]),
    (2, 2, &[0, 1, 2, 3]),
    (3, 2, &[0, 4, 1, 2, 3]),
    (4, 2, &[0, 4, 1, 2, 3, 5]),
    (5, 2, &[0, 4, 1, 2, 3, 5, 6]),
    (5, 3, &[0, 6, 1, 2, 3, 4, 5, 7]),
];

/// An Opus encoder for 1 to 8 channels at 48 kHz, in Vorbis channel order.
#[derive(Debug, Clone)]
pub struct MultistreamEncoder {
    /// The first `coupled` stereo, the rest mono.
    streams: Vec<Encoder>,
    coupled: usize,
    /// Per input channel, the coded channel it feeds.
    table: &'static [u8],
    bitrate: u32,
    /// One stream's input and packet, reused.
    pcm: Vec<f32>,
    packet: Vec<u8>,
}

impl MultistreamEncoder {
    /// An encoder for `channels` (1 to 8) channels: 20 ms frames at 64 kb/s a channel.
    pub fn new(channels: usize) -> Result<Self> {
        if !(1..=8).contains(&channels) {
            return Err(Error::BadArgument(
                "a Vorbis-order Opus stream has one to eight channels",
            ));
        }
        let (streams, coupled, table) = VORBIS_MAPPINGS[channels - 1];
        let coupled = usize::from(coupled);
        let streams = (0..usize::from(streams))
            .map(|s| Encoder::new(if s < coupled { 2 } else { 1 }))
            .collect::<Result<Vec<_>>>()?;
        let mut e = Self {
            streams,
            coupled,
            table,
            bitrate: 0,
            pcm: Vec::new(),
            packet: Vec::new(),
        };
        e.set_bitrate(64_000 * channels as u32)?;
        Ok(e)
    }

    pub fn channels(&self) -> usize {
        self.table.len()
    }

    /// The bitrate of the whole stream in bits per second.
    pub fn bitrate(&self) -> u32 {
        self.bitrate
    }

    /// Sets the bitrate of the whole stream, shared between the streams by their channels:
    /// 6,000 to 510,000 bits per second for each.
    pub fn set_bitrate(&mut self, bits_per_second: u32) -> Result<()> {
        let channels = self.table.len() as u64;
        let share = |s: usize| {
            (u64::from(bits_per_second) * self.stream_channels(s) as u64 / channels) as u32
        };
        // Checked for every stream before any is changed.
        let mut probe = self.streams.clone();
        for (s, e) in probe.iter_mut().enumerate() {
            e.set_bitrate(share(s))?;
        }
        self.streams = probe;
        self.bitrate = bits_per_second;
        Ok(())
    }

    /// See [`Encoder::set_vbr`].
    pub fn set_vbr(&mut self, vbr: bool) {
        for e in &mut self.streams {
            e.set_vbr(vbr);
        }
    }

    /// Samples per channel that [`encode`](Self::encode) takes.
    pub fn frame_size(&self) -> usize {
        self.streams[0].frame_size()
    }

    /// See [`Encoder::set_frame_size`].
    pub fn set_frame_size(&mut self, samples: usize) -> Result<()> {
        let mut probe = self.streams.clone();
        for e in &mut probe {
            e.set_frame_size(samples)?;
        }
        self.streams = probe;
        Ok(())
    }

    /// Samples by which the decoded signal trails the input: the header's pre-skip.
    pub fn lookahead(&self) -> usize {
        self.streams[0].lookahead()
    }

    /// Each stream's range after the last packet, as `MultistreamDecoder::final_ranges`.
    pub fn final_ranges(&self) -> Vec<u32> {
        self.streams.iter().map(Encoder::final_range).collect()
    }

    /// Forgets the stream; the settings stay.
    pub fn reset(&mut self) {
        for e in &mut self.streams {
            e.reset();
        }
    }

    fn stream_channels(&self, s: usize) -> usize {
        if s < self.coupled { 2 } else { 1 }
    }

    /// The identification header of the stream this encoder writes: family 0 for mono and
    /// stereo, family 1 above.
    pub fn head(&self, input_sample_rate: u32) -> OpusHead {
        let channels = self.table.len() as u8;
        OpusHead {
            version: 1,
            channels,
            pre_skip: self.lookahead() as u16,
            input_sample_rate,
            output_gain: 0,
            mapping: ChannelMapping {
                family: u8::from(channels > 2),
                streams: self.streams.len() as u8,
                coupled: self.coupled as u8,
                table: self.table.to_vec(),
            },
        }
    }

    /// Encodes one frame, `frame_size * channels` interleaved samples in Vorbis channel
    /// order, and appends its packet to `out`. On an error nothing is appended.
    pub fn encode(&mut self, pcm: &[f32], out: &mut Vec<u8>) -> Result<()> {
        let channels = self.table.len();
        let frames = self.frame_size();
        if pcm.len() != frames * channels {
            return Err(Error::BadArgument("an Opus frame of the wrong length"));
        }
        let start = out.len();
        let n = self.streams.len();
        for s in 0..n {
            let width = self.stream_channels(s);
            // The input channels that feed this stream's coded channels.
            let first = if s < self.coupled {
                2 * s
            } else {
                s + self.coupled
            };
            let source = |coded: usize| {
                self.table
                    .iter()
                    .position(|&m| usize::from(m) == coded)
                    .expect("every coded channel of a Vorbis mapping has its input")
            };
            let from = [source(first), source(first + width - 1)];
            self.pcm.clear();
            for frame in pcm.chunks_exact(channels) {
                for &ch in &from[..width] {
                    self.pcm.push(frame[ch]);
                }
            }
            self.packet.clear();
            if let Err(e) = self.streams[s].encode(&self.pcm, &mut self.packet) {
                out.truncate(start);
                return Err(e);
            }
            if s + 1 < n {
                // Self-delimited: a code 0 packet's one frame gets its length after the TOC.
                let len = self.packet.len() - 1;
                out.push(self.packet[0]);
                if len < 252 {
                    out.push(len as u8);
                } else {
                    let low = 252 + (len & 3);
                    out.push(low as u8);
                    out.push(((len - low) / 4) as u8);
                }
                out.extend_from_slice(&self.packet[1..]);
            } else {
                out.extend_from_slice(&self.packet);
            }
        }
        Ok(())
    }
}
