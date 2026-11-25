//! Multistream encoding: a port of libopus `src/opus_multistream_encoder.c` (3-clause BSD, see
//! the crate's `COPYING`) for mapping families 0 and 1: the Vorbis layouts of 1 to 8
//! channels, the per-stream rate allocation of the surround encoder, the bandwidth it sets
//! from the total rate, coupled streams forced to stereo CELT, the LFE stream in CELT's LFE
//! mode, and every stream's packet but the last self-delimited (RFC 6716 Appendix B).
//!
//! Not ported: the surround masking analysis (`surround_analysis` and the energy masks it
//! hands each stream), which in libopus lowers the bits of channels another masks; here each
//! stream is coded on its own at its allocated rate.

use crate::encoder::{Encoder, EncoderConfig};
use crate::error::{Error, Result};
use crate::head::{ChannelMapping, OpusHead};
use crate::packet::{Bandwidth, Mode};
use crate::repacketizer::reframe;

/// `vorbis_mappings`: streams, coupled streams and the table, per channel count.
const VORBIS_MAPPINGS: [(u8, u8, &[u8]); 8] = [
    (1, 0, &[0]),
    (1, 1, &[0, 1]),
    (2, 1, &[0, 2, 1]),
    (2, 2, &[0, 1, 2, 3]),
    (3, 2, &[0, 4, 1, 2, 3]),
    (4, 2, &[0, 4, 1, 2, 3, 5]),
    (4, 3, &[0, 4, 1, 2, 3, 5, 6]),
    (5, 3, &[0, 6, 1, 2, 3, 4, 5, 7]),
];

/// The most a stream's packet may take (`MS_FRAME_TMP`).
const MS_FRAME_TMP: usize = 6 * 1275 + 12;

/// An Opus multistream encoder: 48 kHz PCM of 1 to 8 channels in the Vorbis order (RFC 7845
/// 5.1.1.2: mono; L R; L C R; FL FR RL RR; FL C FR RL RR; FL C FR RL RR LFE; FL C FR SL SR RC
/// LFE; FL C FR SL SR RL RR LFE) in, one multistream packet per call out.
#[derive(Debug, Clone)]
pub struct MultistreamEncoder {
    config: EncoderConfig,
    mapping: ChannelMapping,
    /// One per stream, the coupled ones first.
    streams: Vec<Encoder>,
    lfe_stream: Option<usize>,
    surround: bool,
    /// Per stream, its input channels (one or two).
    inputs: Vec<(usize, Option<usize>)>,
    buf: Vec<f32>,
}

impl MultistreamEncoder {
    /// An encoder for `config` (its `channels` up to 8, its `bitrate` the total) in mapping
    /// `family` 0 (one or two channels, one stream) or 1 (the Vorbis layouts). The other
    /// settings apply to every stream, except that from 3 channels on, as in libopus, each
    /// stream's bandwidth follows the total rate, the coupled streams are stereo CELT and the
    /// LFE (6 channels and up) is CELT's LFE mode.
    pub fn new(config: &EncoderConfig, family: u8) -> Result<Self> {
        let channels = usize::from(config.channels);
        let (streams, coupled, table) = match (family, channels) {
            (0, 1 | 2) | (1, 1..=8) => VORBIS_MAPPINGS[channels - 1],
            (0, _) => return Err(Error::BadArgument("mapping family 0 has 1 or 2 channels")),
            (1, _) => return Err(Error::BadArgument("mapping family 1 has 1 to 8 channels")),
            _ => {
                return Err(Error::Unsupported(
                    "only mapping families 0 and 1 are encoded",
                ));
            }
        };
        if config.bitrate == 0 {
            return Err(Error::BadArgument("a bitrate of 0"));
        }
        let mapping = ChannelMapping {
            family,
            streams,
            coupled,
            table: table.to_vec(),
        };
        let (streams, coupled) = (usize::from(streams), usize::from(coupled));
        let lfe_stream = (family == 1 && channels >= 6).then_some(streams - 1);
        let mut encs = Vec::with_capacity(streams);
        let mut inputs = Vec::with_capacity(streams);
        for s in 0..streams {
            let stereo = s < coupled;
            let cfg = EncoderConfig {
                channels: if stereo { 2 } else { 1 },
                ..config.clone()
            };
            let mut enc = Encoder::new(&cfg)?;
            if lfe_stream == Some(s) {
                enc.set_lfe(true);
            }
            encs.push(enc);
            let find = |want: usize| table.iter().position(|&m| usize::from(m) == want);
            let input = if stereo {
                (find(2 * s), find(2 * s + 1))
            } else {
                (find(s + coupled), None)
            };
            let first = input
                .0
                .ok_or(Error::BadArgument("a stream with no input channel"))?;
            inputs.push((first, input.1));
        }
        Ok(Self {
            config: config.clone(),
            mapping,
            streams: encs,
            lfe_stream,
            surround: family == 1 && channels > 2,
            inputs,
            buf: Vec::new(),
        })
    }

    /// Samples per channel in each packet.
    pub fn frame_size(&self) -> usize {
        self.config.frame_duration.samples()
    }

    /// Samples (at 48 kHz) by which decoded output lags the input: the pre-skip.
    pub fn lookahead(&self) -> u32 {
        self.streams[0].lookahead()
    }

    /// The channel mapping the packets need, as the `OpusHead` carries it.
    pub fn mapping(&self) -> &ChannelMapping {
        &self.mapping
    }

    /// The `OpusHead` for a stream of these packets (pre-skip from the lookahead, no gain),
    /// recording `input_sample_rate` (informational: what the audio was before resampling to
    /// 48 kHz, 0 if unknown).
    pub fn head(&self, input_sample_rate: u32) -> OpusHead {
        OpusHead {
            version: 1,
            channels: self.config.channels,
            pre_skip: self.lookahead() as u16,
            input_sample_rate,
            output_gain: 0,
            mapping: self.mapping.clone(),
        }
    }

    /// Each stream's final range after the last packet, in stream order (what
    /// `MultistreamDecoder::final_ranges` reports).
    pub fn final_ranges(&self) -> Vec<u32> {
        self.streams.iter().map(Encoder::final_range).collect()
    }

    /// Changes the total bitrate from the next packet on; 0 is refused.
    pub fn set_bitrate(&mut self, bitrate: u32) -> Result<()> {
        if bitrate == 0 {
            return Err(Error::BadArgument("a bitrate of 0"));
        }
        self.config.bitrate = bitrate;
        Ok(())
    }

    /// Forgets the stream, as if new.
    pub fn reset(&mut self) {
        for s in &mut self.streams {
            s.reset();
        }
    }

    /// `surround_rate_allocation`: each stream's bitrate out of the total.
    fn rate_allocation(&self, frame_size: usize) -> Vec<i32> {
        let nb_lfe = i32::from(self.lfe_stream.is_some());
        let nb_coupled = i32::from(self.mapping.coupled);
        let nb_uncoupled = i32::from(self.mapping.streams) - nb_coupled - nb_lfe;
        let nb_normal = 2 * nb_coupled + nb_uncoupled;
        let frame_rate = (48_000 / frame_size) as i32;
        // Enough per non-LFE channel to code the band energies.
        let channel_offset = 40 * frame_rate.max(50);
        let bitrate = self.config.bitrate.min(i32::MAX as u32) as i32;
        // Some for the LFE, but never more than a twentieth of the total.
        let lfe_offset = (bitrate / 20).min(3000) + 15 * frame_rate.max(50);
        // Each stream gets a start, the saving of coupling.
        let stream_offset =
            ((bitrate - channel_offset * nb_normal - lfe_offset * nb_lfe) / nb_normal.max(1) / 2)
                .clamp(0, 20_000);
        // Coupled streams twice a mono's rate past their offsets, the LFE an eighth.
        let coupled_ratio = 512;
        let lfe_ratio = 32;
        let total = (nb_uncoupled << 8) + coupled_ratio * nb_coupled + nb_lfe * lfe_ratio;
        let channel_rate =
            (256 * i64::from(
                bitrate
                    - lfe_offset * nb_lfe
                    - stream_offset * (nb_coupled + nb_uncoupled)
                    - channel_offset * nb_normal,
            ) / i64::from(total.max(1))) as i32;
        (0..usize::from(self.mapping.streams))
            .map(|i| {
                let r = if i < usize::from(self.mapping.coupled) {
                    2 * channel_offset
                        + (stream_offset + ((channel_rate * coupled_ratio) >> 8)).max(0)
                } else if Some(i) != self.lfe_stream {
                    channel_offset + (stream_offset + channel_rate).max(0)
                } else {
                    (lfe_offset + ((channel_rate * lfe_ratio) >> 8)).max(0)
                };
                r.max(500)
            })
            .collect()
    }

    /// Encodes exactly [`frame_size`](Self::frame_size) samples per channel, interleaved in
    /// the Vorbis order, full scale 1.0, into `packet` (cleared first).
    pub fn encode(&mut self, pcm: &[f32], packet: &mut Vec<u8>) -> Result<()> {
        let channels = usize::from(self.config.channels);
        let frame_size = self.frame_size();
        if pcm.len() != frame_size * channels {
            return Err(Error::BadArgument(
                "an Opus encoder takes one frame of interleaved samples",
            ));
        }
        packet.clear();
        let nb_streams = self.streams.len();
        let frame_rate = (48_000 / frame_size) as i32;
        let smallest_packet = nb_streams * 2 - 1;
        let mut max_data_bytes = nb_streams * MS_FRAME_TMP;
        let rates = self.rate_allocation(frame_size);
        let vbr = self.config.vbr;
        let total = self.config.bitrate.min(i32::MAX as u32) as i32;
        if !vbr {
            let cbr = (3 * i64::from(total) / (3 * 8 * i64::from(frame_rate))) as usize;
            max_data_bytes = max_data_bytes.min(smallest_packet.max(cbr));
        }
        for (s, enc) in self.streams.iter_mut().enumerate() {
            enc.set_bitrate(rates[s] as u32)?;
            if self.surround {
                let mut equiv = total;
                if frame_size * 50 < 48_000 {
                    equiv -= 60 * (frame_rate - 50) * channels as i32;
                }
                let ch = channels as i32;
                let bw = if equiv > 10_000 * ch {
                    Bandwidth::Full
                } else if equiv > 7000 * ch {
                    Bandwidth::SuperWide
                } else if equiv > 5000 * ch {
                    Bandwidth::Wide
                } else {
                    Bandwidth::Narrow
                };
                // Coupled streams stay stereo CELT, for the spatial image.
                let coupled = s < usize::from(self.mapping.coupled);
                let mode = if coupled {
                    Some(Mode::Celt)
                } else {
                    self.config.mode
                };
                enc.set_mode_and_bandwidth(mode, Some(bw));
                if coupled {
                    enc.set_force_channels(Some(2));
                }
            }
        }

        let mut tot_size = 0;
        let mut stream_packet = Vec::new();
        for s in 0..nb_streams {
            let (a, b) = self.inputs[s];
            self.buf.clear();
            for frame in pcm.chunks_exact(channels) {
                self.buf.push(frame[a]);
                if let Some(b) = b {
                    self.buf.push(frame[b]);
                }
            }
            let last = s == nb_streams - 1;
            // Room left, keeping one byte for the last stream and two for each other.
            let mut curr_max = max_data_bytes.saturating_sub(tot_size);
            curr_max = curr_max.saturating_sub((2 * (nb_streams - s - 1)).saturating_sub(1));
            curr_max = curr_max.min(MS_FRAME_TMP);
            // The self-delimiting length takes one or two bytes.
            if !last {
                curr_max = curr_max.saturating_sub(if curr_max > 253 { 2 } else { 1 });
            }
            let enc = &mut self.streams[s];
            if !vbr && last {
                enc.set_bitrate((curr_max as i64 * 8 * i64::from(frame_rate)).max(1) as u32)?;
            }
            stream_packet.clear();
            enc.encode_native(&self.buf, curr_max.max(1), &mut stream_packet)?;
            let framed = reframe(
                &stream_packet,
                max_data_bytes - tot_size,
                !last,
                !vbr && last,
            )?;
            tot_size += framed.len();
            packet.extend_from_slice(&framed);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "multistream_enc_tests.rs"]
mod tests;
