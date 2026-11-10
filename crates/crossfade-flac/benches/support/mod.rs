//! Input shared by the bench and the profiling example: a synthetic signal encoded once through
//! the engine's FLAC encoder, returned as STREAMINFO plus pre-split frames.
use crossfade_codec::{EncodeOptions, encoder_for};
use crossfade_core::{
    AudioFrame, AudioSpec, ChannelLayout, CodecId, Packet, SampleFormat, Samples,
};
use crossfade_flac::streaminfo::StreamInfoBlock;

pub(crate) struct Input {
    pub(crate) name: &'static str,
    pub(crate) pcm_bytes: u64,
    pub(crate) info: StreamInfoBlock,
    pub(crate) frames: Vec<Vec<u8>>,
}

/// A sine mix plus seeded noise, so the residual is realistic.
pub(crate) fn input(name: &'static str, rate: u32, format: SampleFormat, seconds: u32) -> Input {
    let bits = format.bits() as u32;
    let amp = f64::from((1u32 << (bits - 1)) - 1);
    let mut state = 0x9e37_79b9u32;
    let mut samples = Vec::new();
    for t in 0..rate as usize * seconds as usize {
        for ch in 0..2 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = f64::from(state >> 8) / f64::from(1u32 << 24) - 0.5;
            let x = t as f64;
            let tone = (x * (0.06 + 0.013 * f64::from(ch))).sin() * 0.4
                + (x * 0.0007).sin() * 0.25
                + (x * 0.0213).sin() * 0.1;
            samples.push(((tone + noise * 0.02) * amp) as i32);
        }
    }
    let spec = AudioSpec::new(rate, format, ChannelLayout::stereo()).unwrap();
    let mut frame = AudioFrame::new(&spec);
    frame.samples = match format {
        SampleFormat::S16 => Samples::S16(samples.iter().map(|&v| v as i16).collect()),
        _ => Samples::S24(samples.clone()),
    };
    let mut enc = encoder_for(
        CodecId::Flac,
        &spec,
        &EncodeOptions {
            compression_level: None,
        },
    )
    .unwrap();
    enc.send_frame(&frame).unwrap();
    enc.drain();
    let mut frames = Vec::new();
    let mut p = Packet::default();
    while enc.receive_packet(&mut p).unwrap() {
        frames.push(p.data.clone());
    }
    Input {
        name,
        pcm_bytes: (samples.len() * format.bytes_per_sample()) as u64,
        info: StreamInfoBlock::parse(enc.extradata()).unwrap(),
        frames,
    }
}
