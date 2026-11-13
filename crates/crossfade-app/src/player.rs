//! Preview playback (skeleton: the API the tests are written against; nothing plays yet).

mod sink;

pub use sink::{VirtualOutput, VirtualSink};

use crossfade_core::{ChannelLayout, Error, JobIo, Limits, StreamInfo};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputFormat {
    pub sample_rate: u32,
    pub channels: u16,
}

pub trait AudioSink {
    fn format(&self) -> OutputFormat;
    fn connect(&mut self, feed: Feed) -> Result<(), String>;
    fn start(&mut self) -> Result<(), String>;
    fn pause(&mut self) -> Result<(), String>;
    fn stop(&mut self);
}

pub struct Feed;

impl Feed {
    pub fn fill(&mut self, out: &mut [f32]) {
        out.fill(0.0);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Empty,
    Loading,
    Ready,
    Playing,
    Paused,
    Ended,
    Failed(String),
}

pub fn device_layout(channels: u16) -> Result<ChannelLayout, Error> {
    ChannelLayout::default_for(channels)
}

pub struct Player;

impl Player {
    pub fn new(
        _: Box<dyn AudioSink>,
        _: Box<dyn JobIo + Send>,
        _: Limits,
        _: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, String> {
        Ok(Self)
    }
    pub fn format(&self) -> OutputFormat {
        OutputFormat { sample_rate: 0, channels: 0 }
    }
    pub fn load(&mut self, _: impl Into<String>) {}
    pub fn play(&mut self) {}
    pub fn pause(&mut self) {}
    pub fn toggle(&mut self) {}
    pub fn stop(&mut self) {}
    pub fn seek(&mut self, _: u64) {}
    pub fn scrub_to(&mut self, _: u64) {}
    pub fn end_scrub(&mut self) {}
    pub fn is_scrubbing(&self) -> bool {
        false
    }
    pub fn state(&self) -> State {
        State::Empty
    }
    pub fn position(&self) -> u64 {
        0
    }
    pub fn duration(&self) -> Option<u64> {
        None
    }
    pub fn stream(&self) -> Option<StreamInfo> {
        None
    }
    pub fn underruns(&self) -> u64 {
        0
    }
    pub fn pulled(&self) -> u64 {
        0
    }
    pub fn settle(&self) {}
}

pub fn clock_text(_: u64, _: u32) -> String {
    String::new()
}

pub fn transport_text(_: u64, _: Option<u64>, _: u32) -> String {
    String::new()
}
