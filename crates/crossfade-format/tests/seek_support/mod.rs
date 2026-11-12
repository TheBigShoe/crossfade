//! Shared by the seeking tests: an in-memory sink, a source that counts what it reads, and
//! packet helpers.
#![allow(dead_code, unreachable_pub)]

use crossfade_core::{Demuxer, Error, Limits, MediaSink, MediaSource, Packet};
use std::io::{Cursor, SeekFrom};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct Shared(Arc<Mutex<Cursor<Vec<u8>>>>);

impl Shared {
    pub fn bytes(&self) -> Vec<u8> {
        self.0.lock().unwrap().get_ref().clone()
    }
}

impl MediaSink for Shared {
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        std::io::Write::write_all(&mut *self.0.lock().unwrap(), buf)
    }
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        std::io::Seek::seek(&mut *self.0.lock().unwrap(), pos)
    }
    fn finish(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Bytes read and seeks made through a source, readable while a demuxer owns it.
#[derive(Clone, Default)]
pub struct Counter {
    pub bytes: Arc<AtomicU64>,
    pub seeks: Arc<AtomicU64>,
}

impl Counter {
    pub fn take(&self) -> (u64, u64) {
        (
            self.bytes.swap(0, Ordering::Relaxed),
            self.seeks.swap(0, Ordering::Relaxed),
        )
    }
}

pub struct Counting {
    inner: Cursor<Vec<u8>>,
    counter: Counter,
}

impl Counting {
    pub fn new(bytes: Vec<u8>) -> (Self, Counter) {
        let counter = Counter::default();
        (
            Self {
                inner: Cursor::new(bytes),
                counter: counter.clone(),
            },
            counter,
        )
    }
}

impl MediaSource for Counting {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = std::io::Read::read(&mut self.inner, buf)?;
        self.counter.bytes.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.counter.seeks.fetch_add(1, Ordering::Relaxed);
        std::io::Seek::seek(&mut self.inner, pos)
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.inner.get_ref().len() as u64)
    }
    fn is_seekable(&self) -> bool {
        true
    }
}

pub fn open_bytes(bytes: &[u8]) -> Box<dyn Demuxer> {
    crossfade_format::open_demuxer(Box::new(Cursor::new(bytes.to_vec())), &Limits::default())
        .unwrap()
        .1
}

pub fn drain(d: &mut dyn Demuxer) -> Result<Vec<Packet>, Error> {
    let mut out = Vec::new();
    loop {
        let mut p = Packet::default();
        if !d.read_packet(&mut p)? {
            return Ok(out);
        }
        out.push(p);
    }
}
