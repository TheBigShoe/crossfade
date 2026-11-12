//! ALAC decoder adapter: one packet in, one frame out, channels moved from ALAC's order into
//! the stream layout's (WAVE) order and samples left-justified in the stream's container
//! format (20-bit audio sits in the top of `S24`). The decoding lives in `crossfade-alac`.
//!
//! Packets are independent, so with threads a batch of them decodes on several at once
//! (`std::thread::scope`, one `crossfade_alac::Decoder` each) and the frames come out in
//! order, a batch late; the frames and the errors are the same at any thread count: when a
//! packet fails, every frame before it comes out, then its error, and nothing after. A reset
//! forgets what is queued.

use crossfade_core::{AudioFrame, Decoder, Error, Packet, SampleFormat, Samples, StreamInfo};
use std::collections::VecDeque;

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

/// At most this many threads: decoding outruns the rest of a conversion well before.
const MAX_THREADS: usize = 4;
/// Packets per thread in a batch: enough work (about 2 ms) to pay for starting a thread.
const PACKETS_PER_THREAD: usize = 16;

/// One thread's part of a batch: its decoder, its packets, and where their frames and results go.
type Share<'a> = (
    (&'a mut Worker, &'a [Packet]),
    (&'a mut [AudioFrame], &'a mut [Result<(), Error>]),
);

/// One decoder and its scratch, per thread.
struct Worker {
    inner: crossfade_alac::Decoder,
    decoded: Vec<i32>,
}

/// What every frame looks like.
struct Shape {
    /// For each channel in the frame's order, the ALAC channel it comes from.
    from_alac: &'static [u8],
    /// Left shift into the container.
    justify: u32,
    template: AudioFrame,
}

impl Shape {
    fn fill(&self, decoded: &[i32], pts: i64, frame: &mut AudioFrame) {
        if frame.samples.format() != self.template.samples.format()
            || frame.layout != self.template.layout
        {
            *frame = self.template.clone();
        }
        frame.pts = pts;
        frame.sample_rate = self.template.sample_rate;
        let (order, shift) = (self.from_alac, self.justify);
        match &mut frame.samples {
            Samples::S16(out) => reorder(decoded, order, out, |s| s as i16),
            Samples::S24(out) | Samples::S32(out) => reorder(decoded, order, out, |s| s << shift),
            _ => unreachable!("the template has an integer format"),
        }
    }
}

impl Worker {
    fn decode(
        &mut self,
        shape: &Shape,
        packet: &Packet,
        frame: &mut AudioFrame,
    ) -> Result<(), Error> {
        self.inner
            .decode(&packet.data, &mut self.decoded)
            .map_err(engine_error)?;
        shape.fill(&self.decoded, packet.pts, frame);
        Ok(())
    }
}

struct AlacDecoder {
    workers: Vec<Worker>,
    shape: Shape,
    /// Packets sent and not decoded yet, oldest first.
    queued: Vec<Packet>,
    spare_packets: Vec<Packet>,
    ready: VecDeque<AudioFrame>,
    spare: Vec<AudioFrame>,
    /// A packet failed: nothing after it is decoded.
    failed: bool,
    /// The failure, held back until the frames before it have been received.
    held: Option<Error>,
    draining: bool,
}

pub fn decoder(stream: &StreamInfo) -> Result<Box<dyn Decoder>, Error> {
    decoder_threads(stream, 1)
}

/// As [`decoder`], on up to `threads` threads (0: the machine's parallelism, at most four).
pub fn decoder_threads(stream: &StreamInfo, threads: usize) -> Result<Box<dyn Decoder>, Error> {
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
    let threads = match threads {
        0 => crate::auto_thread_count(),
        n => n,
    }
    .clamp(1, MAX_THREADS);
    let workers = (0..threads)
        .map(|_| {
            Ok(Worker {
                inner: crossfade_alac::Decoder::new(config).map_err(engine_error)?,
                decoded: Vec::new(),
            })
        })
        .collect::<Result<_, Error>>()?;
    Ok(Box::new(AlacDecoder {
        workers,
        shape: Shape {
            from_alac: layout.from_alac,
            justify: width - u32::from(config.bit_depth),
            template: AudioFrame::new(audio),
        },
        queued: Vec::new(),
        spare_packets: Vec::new(),
        ready: VecDeque::new(),
        spare: Vec::new(),
        failed: false,
        held: None,
        draining: false,
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

impl AlacDecoder {
    fn batch(&self) -> usize {
        match self.workers.len() {
            1 => 1,
            n => n * PACKETS_PER_THREAD,
        }
    }

    /// Decodes every queued packet, in order, onto the ready frames.
    fn run(&mut self) -> Result<(), Error> {
        let n = self.queued.len();
        if n == 0 {
            return Ok(());
        }
        let mut frames: Vec<AudioFrame> = (0..n)
            .map(|_| {
                self.spare
                    .pop()
                    .unwrap_or_else(|| self.shape.template.clone())
            })
            .collect();
        let mut results: Vec<Result<(), Error>> = vec![Ok(()); n];
        let per = n.div_ceil(self.workers.len());
        let shape = &self.shape;
        let jobs = self
            .workers
            .iter_mut()
            .zip(self.queued.chunks(per))
            .zip(frames.chunks_mut(per).zip(results.chunks_mut(per)));
        let work = |((w, packets), (frames, results)): Share<'_>| {
            for ((p, f), r) in packets.iter().zip(frames).zip(results) {
                *r = w.decode(shape, p, f);
                if r.is_err() {
                    // Later packets of this share are never reported; skip them.
                    break;
                }
            }
        };
        if per == n {
            jobs.for_each(work);
        } else {
            std::thread::scope(|s| {
                let mut jobs = jobs;
                let first = jobs.next();
                for job in jobs {
                    s.spawn(move || work(job));
                }
                if let Some(job) = first {
                    work(job);
                }
            });
        }
        self.spare_packets.append(&mut self.queued);
        let mut outcome = Ok(());
        for (frame, result) in frames.into_iter().zip(results) {
            if self.failed {
                self.spare.push(frame);
                continue;
            }
            match result {
                Ok(()) => self.ready.push_back(frame),
                Err(e) => {
                    self.failed = true;
                    self.spare.push(frame);
                    if self.ready.is_empty() {
                        outcome = Err(e);
                    } else {
                        self.held = Some(e);
                    }
                }
            }
        }
        outcome
    }
}

impl Decoder for AlacDecoder {
    fn send_packet(&mut self, packet: &Packet) -> Result<(), Error> {
        if self.failed {
            return Ok(());
        }
        let mut p = self.spare_packets.pop().unwrap_or_default();
        p.clone_from(packet);
        self.queued.push(p);
        if self.queued.len() >= self.batch() {
            self.run()?;
        }
        Ok(())
    }

    fn receive_frame(&mut self, out: &mut AudioFrame) -> Result<bool, Error> {
        if self.ready.is_empty() && self.draining {
            self.run()?;
        }
        if let Some(mut frame) = self.ready.pop_front() {
            std::mem::swap(out, &mut frame);
            self.spare.push(frame);
            return Ok(true);
        }
        match self.held.take() {
            Some(e) => Err(e),
            None => Ok(false),
        }
    }

    fn drain(&mut self) {
        self.draining = true;
    }

    fn flush(&mut self) -> Result<(), Error> {
        self.run()
    }

    fn reset(&mut self) {
        self.spare_packets.append(&mut self.queued);
        self.spare.extend(self.ready.drain(..));
        self.failed = false;
        self.held = None;
        self.draining = false;
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

    /// Every frame and the first error, sent one packet at a time as the pipeline does.
    fn outcome(
        s: &StreamInfo,
        packets: &[Packet],
        threads: usize,
    ) -> (Vec<AudioFrame>, Option<Error>) {
        let mut d = decoder_threads(s, threads).unwrap();
        let mut f = AudioFrame::new(&s.audio);
        let mut out = Vec::new();
        let result = (|| {
            for p in packets {
                d.send_packet(p)?;
                while d.receive_frame(&mut f)? {
                    out.push(f.clone());
                }
            }
            d.drain();
            while d.receive_frame(&mut f)? {
                out.push(f.clone());
            }
            Ok(())
        })();
        (out, result.err())
    }

    #[test]
    fn threads_never_change_the_frames_or_the_error() {
        let s = stream(24, 6);
        let good: Vec<Packet> = (0..150)
            .map(|i| packet(24, 6, 1 + i % 7, i64::from(i) * 4096))
            .collect();
        let one = outcome(&s, &good, 1);
        assert_eq!(one.0.len(), 150);
        assert_eq!(one.1, None);
        let mut bad = good.clone();
        bad[77].data = vec![0xE0];
        let want = outcome(&s, &bad, 1);
        assert_eq!(want.0.len(), 77);
        assert!(want.1.is_some());
        for threads in [0, 2, 3, 4, 9] {
            assert!(outcome(&s, &good, threads) == one, "{threads}");
            assert!(outcome(&s, &bad, threads) == want, "{threads}");
        }
    }

    #[test]
    fn flush_and_reset_with_threads() {
        let s = stream(16, 2);
        let mut d = decoder_threads(&s, 3).unwrap();
        let mut f = AudioFrame::new(&s.audio);
        d.send_packet(&packet(16, 2, 2, 0)).unwrap();
        assert!(!d.receive_frame(&mut f).unwrap(), "a batch late");
        d.flush().unwrap();
        assert!(d.receive_frame(&mut f).unwrap());
        d.send_packet(&packet(16, 2, 2, 8)).unwrap();
        d.reset();
        d.drain();
        assert!(!d.receive_frame(&mut f).unwrap());
        d.send_packet(&Packet {
            data: vec![0xE0],
            ..Packet::default()
        })
        .unwrap();
        assert!(d.flush().is_err());
        d.send_packet(&packet(16, 2, 2, 8)).unwrap();
        d.reset();
        d.send_packet(&packet(16, 2, 2, 16)).unwrap();
        d.flush().unwrap();
        assert!(d.receive_frame(&mut f).unwrap());
        assert_eq!(f.pts, 16);
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
        // A failed stream is over: what follows is not decoded.
        assert_eq!(d.send_packet(&cce), Ok(()));
        let mut fresh = decoder(&s).unwrap();
        assert!(matches!(
            fresh.send_packet(&cce),
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
