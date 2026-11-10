//! FLAC container. The reader and the writer live in separate files.

mod demux;
mod mux;

pub use demux::open;
pub use mux::create;

use crossfade_codec::flac::streaminfo::StreamInfoBlock;
use crossfade_core::{AudioSpec, ChannelLayout, CodecId, Error, StreamInfo, TimeBase};

#[cfg(test)]
mod tests {
    use super::*;
    use crossfade_core::{Channel::*, SampleFormat};

    fn block(rate: u32, channels: u8, bits: u8, total: u64) -> StreamInfoBlock {
        StreamInfoBlock {
            min_block_size: 4096,
            max_block_size: 4096,
            min_frame_size: 0,
            max_frame_size: 0,
            sample_rate: rate,
            channels,
            bits_per_sample: bits,
            total_samples: total,
            md5: [0; 16],
        }
    }

    fn info(b: &StreamInfoBlock) -> StreamInfo {
        stream_info(b, &b.write()).unwrap()
    }

    #[test]
    fn basic_fields() {
        let b = block(44100, 2, 16, 1000);
        let s = info(&b);
        assert_eq!((s.index, s.codec), (0, CodecId::Flac));
        assert_eq!(s.time_base, TimeBase::new(1, 44100).unwrap());
        assert_eq!(s.frames, Some(1000));
        assert_eq!(s.extradata, b.write().to_vec());
        assert_eq!(s.audio.sample_rate(), 44100);
        assert_eq!(s.audio.layout(), &ChannelLayout::stereo());
        assert_eq!(s.bits_per_raw_sample, None);
    }

    #[test]
    fn an_unknown_total_is_none() {
        assert_eq!(info(&block(8000, 1, 16, 0)).frames, None);
    }

    #[test]
    fn depth_picks_the_smallest_container_and_records_odd_depths() {
        use SampleFormat::*;
        for (bits, format, raw) in [
            (4, U8, Some(4)),
            (8, U8, None),
            (9, S16, Some(9)),
            (12, S16, Some(12)),
            (16, S16, None),
            (17, S24, Some(17)),
            (20, S24, Some(20)),
            (24, S24, None),
            (25, S32, Some(25)),
            (32, S32, None),
        ] {
            let s = info(&block(48000, 2, bits, 1));
            assert_eq!(s.audio.format(), format, "{bits}");
            assert_eq!(s.bits_per_raw_sample, raw, "{bits}");
        }
    }

    #[test]
    fn channel_orders_follow_rfc_9639() {
        let positioned: [(u8, &[crossfade_core::Channel]); 6] = [
            (1, &[FrontCenter]),
            (3, &[FrontLeft, FrontRight, FrontCenter]),
            (4, &[FrontLeft, FrontRight, BackLeft, BackRight]),
            (5, &[FrontLeft, FrontRight, FrontCenter, BackLeft, BackRight]),
            (6, &[FrontLeft, FrontRight, FrontCenter, Lfe, BackLeft, BackRight]),
            (8, &[FrontLeft, FrontRight, FrontCenter, Lfe, BackLeft, BackRight, SideLeft, SideRight]),
        ];
        for (n, want) in positioned {
            let s = info(&block(48000, n, 16, 1));
            assert_eq!(s.audio.layout().positions(), Some(want), "{n}");
        }
        // 7 channels need a back-centre position, which `Channel` lacks.
        let seven = info(&block(48000, 7, 16, 1));
        assert_eq!(seven.audio.layout(), &ChannelLayout::unpositioned(7).unwrap());
        assert_eq!(info(&block(48000, 2, 16, 1)).audio.layout().channels(), 2);
    }

    #[test]
    fn the_extradata_is_passed_through_as_given() {
        let b = block(48000, 2, 16, 1);
        assert_eq!(stream_info(&b, &[1, 2, 3]).unwrap().extradata, [1, 2, 3]);
    }
}
