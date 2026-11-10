use super::FRAMES_PER_PACKET;
use crossfade_core::{
    AudioSpec, ChannelLayout, CodecId, Demuxer, Error, Limits, MediaSource, Packet, SampleFormat,
    StreamInfo, TimeBase,
};

pub struct WavDemuxer {
    source: Box<dyn MediaSource>,
    streams: [StreamInfo; 1],
    bytes_per_frame: usize,
    next_frame: u64,
    frames_left: u64,
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

impl WavDemuxer {
    pub fn new(mut source: Box<dyn MediaSource>, _limits: &Limits) -> Result<Self, Error> {
        let mut head = [0u8; 12];
        source.read(&mut head)?;
        let mut fmt = None;
        loop {
            let mut ch = [0u8; 8];
            source.read(&mut ch)?;
            let size = u32_at(&ch, 4) as usize;
            let mut body = vec![0u8; size];
            source.read(&mut body)?;
            match &ch[..4] {
                b"fmt " => fmt = Some(body),
                b"data" => {
                    let f = fmt.ok_or(Error::UnrecognisedFormat)?;
                    let format = match (u16_at(&f, 0), u16_at(&f, 14)) {
                        (1, 8) => SampleFormat::U8,
                        (1, 16) => SampleFormat::S16,
                        (1, 24) => SampleFormat::S24,
                        (1, 32) => SampleFormat::S32,
                        (3, 32) => SampleFormat::F32,
                        _ => SampleFormat::F64,
                    };
                    let channels = u16_at(&f, 2);
                    let rate = u32_at(&f, 4);
                    let layout = match channels {
                        1 => ChannelLayout::mono(),
                        2 => ChannelLayout::stereo(),
                        n => ChannelLayout::unpositioned(n)?,
                    };
                    let audio = AudioSpec::new(rate, format, layout)?;
                    let bytes_per_frame = audio.bytes_per_frame();
                    let frames = (size / bytes_per_frame) as u64;
                    // Rewind to the start of the sample data is not needed:
                    // the body was already consumed, so keep it as the source.
                    let stream = StreamInfo {
                        index: 0,
                        codec: CodecId::pcm_for(format),
                        audio,
                        time_base: TimeBase::new(1, i64::from(rate))?,
                        frames: Some(frames),
                    };
                    source = Box::new(std::io::Cursor::new(body));
                    return Ok(Self {
                        source,
                        streams: [stream],
                        bytes_per_frame,
                        next_frame: 0,
                        frames_left: frames,
                    });
                }
                _ => {}
            }
            if size % 2 == 1 {
                source.read(&mut [0u8; 1])?;
            }
        }
    }
}

impl Demuxer for WavDemuxer {
    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }

    fn read_packet(&mut self, out: &mut Packet) -> Result<bool, Error> {
        if self.frames_left == 0 {
            return Ok(false);
        }
        let frames = self.frames_left.min(FRAMES_PER_PACKET as u64);
        out.data.clear();
        out.data.resize(frames as usize * self.bytes_per_frame, 0);
        self.source.read(&mut out.data)?;
        out.stream = 0;
        out.pts = self.next_frame as i64;
        out.duration = frames;
        self.next_frame += frames;
        self.frames_left -= frames;
        Ok(true)
    }
}
