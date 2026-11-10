//! Helpers shared by the fuzz targets: a small `Limits`, a non-seekable in-memory source,
//! and loops that read or decode a stream to its end or first error.
#![forbid(unsafe_code)]

use crossfade_codec::decoder_for;
use crossfade_core::{AudioFrame, Demuxer, Error, Limits, MediaSink, MediaSource, Packet};
use std::io::{self, Cursor, SeekFrom};
use std::sync::{Arc, Mutex};

/// Small so a hostile length field becomes an error, not an out-of-memory kill.
pub fn limits() -> Limits {
    Limits {
        max_alloc_bytes: 1 << 20,
        max_nesting_depth: 8,
        max_packet_bytes: 1 << 16,
    }
}

/// A pipe: reads only, no length, no seeking.
pub struct Pipe {
    data: Vec<u8>,
    at: usize,
}

impl Pipe {
    pub fn new(data: &[u8]) -> Self {
        Self {
            data: data.to_vec(),
            at: 0,
        }
    }
}

impl MediaSource for Pipe {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let rest = &self.data[self.at..];
        let n = buf.len().min(rest.len());
        buf[..n].copy_from_slice(&rest[..n]);
        self.at += n;
        Ok(n)
    }

    fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "pipe"))
    }

    fn byte_len(&self) -> Option<u64> {
        None
    }

    fn is_seekable(&self) -> bool {
        false
    }
}

/// An in-memory sink the target can read back after the muxer has consumed its half.
#[derive(Clone, Default)]
pub struct SharedSink(Arc<Mutex<Cursor<Vec<u8>>>>);

impl SharedSink {
    pub fn bytes(&self) -> Vec<u8> {
        self.0.lock().unwrap().get_ref().clone()
    }
}

impl MediaSink for SharedSink {
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        MediaSink::write_all(&mut *self.0.lock().unwrap(), buf)
    }

    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        MediaSink::seek(&mut *self.0.lock().unwrap(), pos)
    }

    fn finish(&mut self) -> io::Result<()> {
        MediaSink::finish(&mut *self.0.lock().unwrap())
    }
}

/// Reads packets until the end or the first error. Every successful read consumes input, so
/// `data_len` plus slack bounds the loop; running past it is a hang bug.
pub fn read_all(demux: &mut dyn Demuxer, data_len: usize) -> Result<u64, Error> {
    let mut packet = Packet::default();
    for n in 0..data_len as u64 + 16 {
        if !demux.read_packet(&mut packet)? {
            return Ok(n);
        }
    }
    panic!("read loop did not end");
}

/// Like [`read_all`] and also decodes every packet of stream 0 and drains the decoder.
pub fn decode_all(demux: &mut dyn Demuxer, data_len: usize) -> Result<u64, Error> {
    let stream = demux.streams()[0].clone();
    let mut decoder = decoder_for(&stream)?;
    let (mut packet, mut frame) = (Packet::default(), AudioFrame::new(&stream.audio));
    for n in 0..data_len as u64 + 16 {
        if !demux.read_packet(&mut packet)? {
            decoder.drain();
            while decoder.receive_frame(&mut frame)? {}
            return Ok(n);
        }
        decoder.send_packet(&packet)?;
        while decoder.receive_frame(&mut frame)? {}
    }
    panic!("decode loop did not end");
}
