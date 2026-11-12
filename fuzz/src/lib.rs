//! Helpers shared by the fuzz targets: a small `Limits`, a non-seekable in-memory source,
//! and loops that read or decode a stream to its end or first error.
#![forbid(unsafe_code)]

use crossfade_codec::{decoder_for, decoder_for_threads};
use crossfade_core::{AudioFrame, Demuxer, Error, Limits, MediaSink, MediaSource, Packet};
use std::io::{self, Cursor, SeekFrom};
use std::sync::{Arc, Mutex};

/// Small so a hostile length field becomes an error, not an out-of-memory kill.
pub fn limits() -> Limits {
    Limits {
        max_alloc_bytes: 1 << 20,
        max_nesting_depth: 8,
        max_packet_bytes: 1 << 16,
        // One thread, so a crash reproduces the same way every time.
        max_threads: 1,
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

/// Seeks a few times to positions taken from the input's last bytes (most inside the stream,
/// some anywhere), reading a few packets after each, then reads to the end. A seek may fail;
/// nothing may panic or hang.
pub fn seek_and_read(demux: &mut dyn Demuxer, data: &[u8]) {
    let len = demux.streams()[0].frames.unwrap_or(1 << 20);
    let tail = &data[data.len().saturating_sub(32)..];
    let mut packet = Packet::default();
    for (i, chunk) in tail.chunks(8).enumerate() {
        let mut raw = [0u8; 8];
        raw[..chunk.len()].copy_from_slice(chunk);
        let raw = i64::from_le_bytes(raw);
        let target = if i == 3 {
            raw
        } else {
            (raw.unsigned_abs() % len.saturating_add(64)) as i64 - 16
        };
        if demux.seek(target).is_err() {
            continue;
        }
        for _ in 0..4 {
            if !matches!(demux.read_packet(&mut packet), Ok(true)) {
                break;
            }
        }
    }
    let _ = read_all(demux, data.len());
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

/// Everything a decode shows: the packets read, every frame's pts and samples (as debug text
/// of the sample buffer), and the first error's text, read to the end or the first error as
/// the pipeline reads. With threads the reader may get further before a bad frame's error
/// shows, so compare the frames and the error, not the packets read.
pub fn decode_outcome(
    demux: &mut dyn Demuxer,
    data_len: usize,
    threads: usize,
) -> (u64, Vec<(i64, String)>, Result<(), String>) {
    let stream = demux.streams()[0].clone();
    let mut frames = Vec::new();
    let mut packets = 0;
    let result = (|| -> Result<(), Error> {
        let mut decoder = decoder_for_threads(&stream, threads)?;
        let (mut packet, mut frame) = (Packet::default(), AudioFrame::new(&stream.audio));
        for _ in 0..data_len as u64 + 16 {
            match demux.read_packet(&mut packet) {
                Ok(true) => {}
                Ok(false) => {
                    decoder.drain();
                    while decoder.receive_frame(&mut frame)? {
                        frames.push((frame.pts, format!("{:?}", frame.samples)));
                    }
                    return Ok(());
                }
                Err(e) => {
                    // As the pipeline does: an earlier frame's error comes first.
                    decoder.flush()?;
                    while decoder.receive_frame(&mut frame)? {
                        frames.push((frame.pts, format!("{:?}", frame.samples)));
                    }
                    return Err(e);
                }
            }
            packets += 1;
            decoder.send_packet(&packet)?;
            while decoder.receive_frame(&mut frame)? {
                frames.push((frame.pts, format!("{:?}", frame.samples)));
            }
        }
        panic!("decode loop did not end");
    })();
    (packets, frames, result.map_err(|e| e.to_string()))
}
