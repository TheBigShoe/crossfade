//! ALAC decoder adapter: one packet in, one frame out, channels moved from ALAC's order into
//! the stream layout's (WAVE) order and samples left-justified in the stream's container
//! format (20-bit audio sits in the top of `S24`). The decoding lives in `crossfade-alac`.
//! Packets are independent, so a reset only forgets a frame not yet received.

use crossfade_core::{AudioFrame, Decoder, Error, Packet, SampleFormat, Samples, StreamInfo};

fn engine_error(e: crossfade_alac::Error) -> Error {
    match e {
        crossfade_alac::Error::Unsupported(_) | crossfade_alac::Error::InvalidConfig(_) => {
            Error::Unsupported {
                what: e.to_string(),
            }
        }
        _ => Error::Malformed {
            what: e.to_string(),
        },
    }
}

struct AlacDecoder {
    inner: crossfade_alac::Decoder,
    /// For each channel in the frame's order, the ALAC channel it comes from.
    from_alac: &'static [u8],
    /// Left shift into the container.
    justify: u32,
    template: AudioFrame,
    decoded: Vec<i32>,
    ready: Option<AudioFrame>,
}

pub fn decoder(stream: &StreamInfo) -> Result<Box<dyn Decoder>, Error> {
    let config = crossfade_alac::Config::parse(&stream.extradata).map_err(engine_error)?;
    let layout = crossfade_alac::wave_layout(config.channels).ok_or_else(|| Error::Malformed {
        what: "alac: no layout for the channel count".into(),
    })?;
    let (format, width) = match config.bit_depth {
        16 => (SampleFormat::S16, 16),
        20 | 24 => (SampleFormat::S24, 24),
        _ => (SampleFormat::S32, 32),
    };
    let audio = &stream.audio;
    if audio.format() != format
        || audio.layout().channels() != u16::from(config.channels)
        || (config.sample_rate != 0 && audio.sample_rate() != config.sample_rate)
    {
        return Err(Error::Malformed {
            what: "alac: stream description disagrees with its config".into(),
        });
    }
    Ok(Box::new(AlacDecoder {
        inner: crossfade_alac::Decoder::new(config).map_err(engine_error)?,
        from_alac: layout.from_alac,
        justify: width - u32::from(config.bit_depth),
        template: AudioFrame::new(audio),
        decoded: Vec::new(),
        ready: None,
    }))
}

/// Interleaved `src` (ALAC order) into `dst` in frame order, each sample through `f`.
fn reorder<T>(src: &[i32], from_alac: &[u8], dst: &mut Vec<T>, f: impl Fn(i32) -> T) {
    dst.clear();
    let n = from_alac.len();
    if n == 1
        || from_alac
            .iter()
            .enumerate()
            .all(|(i, &c)| usize::from(c) == i)
    {
        dst.extend(src.iter().map(|&s| f(s)));
        return;
    }
    dst.reserve(src.len());
    for frame in src.chunks_exact(n) {
        dst.extend(from_alac.iter().map(|&c| f(frame[usize::from(c)])));
    }
}

impl Decoder for AlacDecoder {
    fn send_packet(&mut self, packet: &Packet) -> Result<(), Error> {
        self.inner
            .decode(&packet.data, &mut self.decoded)
            .map_err(engine_error)?;
        let mut frame = match self.ready.take() {
            // A buffer the caller swapped in is reused when it has the stream's shape.
            Some(f)
                if f.samples.format() == self.template.samples.format()
                    && f.layout == self.template.layout =>
            {
                f
            }
            _ => self.template.clone(),
        };
        frame.pts = packet.pts;
        frame.sample_rate = self.template.sample_rate;
        let (src, order, shift) = (&self.decoded, self.from_alac, self.justify);
        match &mut frame.samples {
            Samples::S16(out) => reorder(src, order, out, |s| s as i16),
            Samples::S24(out) | Samples::S32(out) => reorder(src, order, out, |s| s << shift),
            _ => unreachable!("built with an integer format"),
        }
        self.ready = Some(frame);
        Ok(())
    }

    fn receive_frame(&mut self, out: &mut AudioFrame) -> Result<bool, Error> {
        match self.ready.as_mut() {
            Some(frame) if !frame.samples.is_empty() => {
                std::mem::swap(out, frame);
                // `out`'s old buffers come back as the next frame's.
                frame.samples.clear();
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn drain(&mut self) {}

    fn reset(&mut self) {
        if let Some(frame) = self.ready.as_mut() {
            frame.samples.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossfade_core::{AudioSpec, Channel::*, ChannelLayout, CodecId, TimeBase};

    fn config(depth: u8, channels: u8) -> crossfade_alac::Config {
        crossfade_alac::Config {
            frame_length: 4096,
            compatible_version: 0,
            bit_depth: depth,
            pb: 40,
            mb: 10,
            kb: 14,
            channels,
            max_run: 255,
            max_frame_bytes: 0,
            avg_bit_rate: 0,
            sample_rate: 48000,
        }
    }

    fn stream(depth: u8, channels: u8) -> StreamInfo {
        let format = match depth {
            16 => SampleFormat::S16,
            20 | 24 => SampleFormat::S24,
            _ => SampleFormat::S32,
        };
        let mask = crossfade_alac::wave_layout(channels).unwrap().mask;
        let layout = ChannelLayout::from_mask(mask, u16::from(channels)).unwrap();
        StreamInfo {
            extradata: config(depth, channels).write().to_vec(),
            ..StreamInfo::new(
                0,
                CodecId::Alac,
                AudioSpec::new(48000, format, layout).unwrap(),
                TimeBase::new(1, 48000).unwrap(),
            )
        }
    }

    #[derive(Default)]
    struct Bits(Vec<u8>, usize);

    impl Bits {
        fn put(&mut self, n: u32, v: u64) {
            for i in (0..n).rev() {
                if self.1.is_multiple_of(8) {
                    self.0.push(0);
                }
                if (v >> i) & 1 != 0 {
                    *self.0.last_mut().unwrap() |= 0x80 >> (self.1 % 8);
                }
                self.1 += 1;
            }
        }
    }

    /// An uncompressed packet of `frames` frames: one SCE per channel, ALAC order, sample
    /// values `channel * 100 + frame`.
    fn packet(depth: u32, channels: u8, frames: u32, pts: i64) -> Packet {
        let mut b = Bits::default();
        for c in 0..channels {
            b.put(3, 0); // SCE
            b.put(16, 0);
            b.put(4, 0b1001); // partial, escape
            b.put(32, u64::from(frames));
            for f in 0..frames {
                let v = i64::from(c) * 100 + i64::from(f) - 50;
                b.put(depth, v as u64 & ((1 << depth) - 1));
            }
        }
        b.put(3, 7);
        Packet {
            pts,
            duration: u64::from(frames),
            data: b.0,
            ..Packet::default()
        }
    }

    fn decode(s: &StreamInfo, packets: &[Packet]) -> Vec<AudioFrame> {
        let mut d = decoder(s).unwrap();
        let mut out = Vec::new();
        for p in packets {
            d.send_packet(p).unwrap();
            let mut f = AudioFrame::new(&s.audio);
            while d.receive_frame(&mut f).unwrap() {
                out.push(f.clone());
            }
        }
        d.drain();
        assert!(!d.receive_frame(&mut AudioFrame::new(&s.audio)).unwrap());
        out
    }

    #[test]
    fn frames_carry_the_packet_pts_and_left_justified_samples() {
        let f = decode(&stream(16, 1), &[packet(16, 1, 3, -7), packet(16, 1, 1, 9)]);
        assert_eq!(f.len(), 2);
        assert_eq!((f[0].pts, f[1].pts), (-7, 9));
        assert_eq!(f[0].samples, Samples::S16(vec![-50, -49, -48]));
        let f = decode(&stream(20, 1), &[packet(20, 1, 2, 0)]);
        assert_eq!(f[0].samples, Samples::S24(vec![-50 << 4, -49 << 4]));
        let f = decode(&stream(32, 1), &[packet(32, 1, 1, 0)]);
        assert_eq!(f[0].samples, Samples::S32(vec![-50]));
    }

    #[test]
    fn channels_move_from_alac_order_to_the_layouts() {
        // 5.1: ALAC C L R Ls Rs LFE (values -50, 50, 150, 250, 350, 450 at frame 0) becomes
        // FL FR FC LFE SL SR.
        let s = stream(24, 6);
        assert_eq!(
            s.audio.layout().positions(),
            Some(&[FrontLeft, FrontRight, FrontCenter, Lfe, SideLeft, SideRight][..])
        );
        let f = decode(&s, &[packet(24, 6, 1, 0)]);
        assert_eq!(
            f[0].samples,
            Samples::S24(vec![50, 150, -50, 450, 250, 350])
        );
        let f = decode(&stream(16, 2), &[packet(16, 2, 1, 0)]);
        assert_eq!(f[0].samples, Samples::S16(vec![-50, 50]));
    }

    #[test]
    fn reset_forgets_a_frame_not_yet_received() {
        let s = stream(16, 1);
        let mut d = decoder(&s).unwrap();
        d.send_packet(&packet(16, 1, 2, 0)).unwrap();
        d.reset();
        let mut f = AudioFrame::new(&s.audio);
        assert!(!d.receive_frame(&mut f).unwrap());
        d.send_packet(&packet(16, 1, 2, 4096)).unwrap();
        assert!(d.receive_frame(&mut f).unwrap());
        assert_eq!(f.pts, 4096);
    }

    #[test]
    fn errors_keep_the_crates_text() {
        let s = stream(16, 1);
        let mut d = decoder(&s).unwrap();
        let bad = Packet {
            data: vec![0xE0],
            ..Packet::default()
        };
        assert_eq!(
            d.send_packet(&bad),
            Err(Error::Malformed {
                what: "alac: packet ends before every channel".into()
            })
        );
        let cce = Packet {
            data: vec![0x40],
            ..Packet::default()
        };
        assert!(matches!(
            d.send_packet(&cce),
            Err(Error::Unsupported { .. })
        ));
        let mut wrong = stream(16, 2);
        wrong.extradata = config(24, 2).write().to_vec();
        assert_eq!(
            decoder(&wrong).err(),
            Some(Error::Malformed {
                what: "alac: stream description disagrees with its config".into()
            })
        );
        let mut short = stream(16, 2);
        short.extradata.truncate(10);
        assert!(matches!(decoder(&short), Err(Error::Unsupported { .. })));
    }
}
