//! `JobIo` over the real file system.

use crossfade_core::{Error, JobIo, MediaSink, MediaSource};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread::JoinHandle;

/// Tells apart two temp files of one process (the pid alone cannot).
static TEMP_SERIAL: AtomicU64 = AtomicU64::new(0);

/// Output goes to a hidden temporary file beside the target; `commit` renames
/// it into place, so a reader never sees a half-written file.
#[derive(Default)]
pub struct FsIo {
    temps: HashMap<String, PathBuf>,
}

fn io_error(path: &str, e: &io::Error) -> Error {
    Error::Io {
        kind: e.kind(),
        message: format!("{path}: {e}"),
    }
}

fn temp_path(target: &Path, serial: u64) -> PathBuf {
    let name = target
        .file_name()
        .map_or_else(|| "output".into(), |n| n.to_string_lossy());
    let pid = std::process::id();
    // The first one keeps the plain name; a clash only happens within a process.
    let temp = if serial == 0 {
        format!(".{name}.{pid}.tmp")
    } else {
        format!(".{name}.{pid}-{serial}.tmp")
    };
    match target.parent() {
        Some(dir) => dir.join(temp),
        None => PathBuf::from(temp),
    }
}

impl FsIo {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Bytes per hand-over between the calling thread and an I/O thread: large
/// enough that system calls are rare, small enough that the first one starts
/// soon.
const CHUNK: usize = 256 * 1024;
/// The read that answers a reader's first request after opening or seeking.
const FIRST_READ: usize = 16 * 1024;
/// Chunks in flight per direction.
const DEPTH: usize = 2;

fn spawn<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> io::Result<JoinHandle<T>> {
    std::thread::Builder::new()
        .name("crossfade-io".into())
        .spawn(work)
}

fn worker_lost() -> io::Error {
    io::Error::other("file I/O thread stopped unexpectedly")
}

/// Reads the file on its own thread, a few chunks ahead, so the kernel's copy
/// overlaps the conversion. A seek stops the thread and starts afresh.
struct FileSource {
    state: ReadState,
    chunk: Vec<u8>,
    at: usize,
    /// Where the caller is, which the file's own offset has run ahead of.
    pos: u64,
    /// Looked up once; devices and pipes report no length and cannot seek.
    byte_len: Option<u64>,
    /// The thread saw the end of the file and has stopped.
    at_end: bool,
    /// Nothing has been read since opening or the last seek.
    fresh: bool,
}

enum ReadState {
    Idle(File),
    Running {
        chunks: Receiver<io::Result<Vec<u8>>>,
        spare: SyncSender<Vec<u8>>,
        thread: JoinHandle<File>,
    },
    /// The thread was lost; nothing more can be read.
    Broken,
}

fn read_ahead(
    mut file: File,
    chunks: SyncSender<io::Result<Vec<u8>>>,
    spare: Receiver<Vec<u8>>,
) -> File {
    loop {
        let mut buf = spare.try_recv().unwrap_or_default();
        buf.resize(CHUNK, 0);
        let mut n = 0;
        let result = loop {
            match Read::read(&mut file, &mut buf[n..]) {
                Ok(0) => break Ok(()),
                Ok(k) => {
                    n += k;
                    if n == buf.len() {
                        break Ok(());
                    }
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => break Err(e),
            }
        };
        buf.truncate(n);
        let last = n == 0 || result.is_err();
        // A short chunk before an error is still data the caller should see.
        if n > 0 && chunks.send(Ok(buf)).is_err() {
            return file;
        }
        let message = match result {
            Err(e) => Err(e),
            Ok(()) if n == 0 => Ok(Vec::new()),
            Ok(()) => continue,
        };
        let _ = chunks.send(message);
        if last {
            return file;
        }
    }
}

impl FileSource {
    fn new(file: File) -> Self {
        let byte_len = file
            .metadata()
            .ok()
            .filter(std::fs::Metadata::is_file)
            .map(|m| m.len());
        Self {
            state: ReadState::Idle(file),
            chunk: Vec::new(),
            at: 0,
            pos: 0,
            byte_len,
            at_end: false,
            fresh: true,
        }
    }

    /// Stops the thread and hands back the file; buffered bytes are dropped.
    fn stop(&mut self) -> io::Result<File> {
        self.chunk.clear();
        self.at = 0;
        match std::mem::replace(&mut self.state, ReadState::Broken) {
            ReadState::Idle(file) => Ok(file),
            ReadState::Running {
                chunks,
                spare,
                thread,
            } => {
                drop((chunks, spare));
                thread.join().map_err(|_| worker_lost())
            }
            ReadState::Broken => Err(worker_lost()),
        }
    }

    fn next_chunk(&mut self) -> io::Result<()> {
        if let ReadState::Idle(file) = &mut self.state {
            // Headers are read in small pieces between seeks, so the first
            // read after opening or seeking is done here: a thread started
            // now would read ahead into what the next seek throws away. Past
            // the end a thread would only find the end again; reading here
            // also lets a pipe or a growing file deliver more.
            if self.at_end || self.fresh {
                self.fresh = false;
                self.chunk
                    .resize(if self.at_end { CHUNK } else { FIRST_READ }, 0);
                let n = loop {
                    match Read::read(file, &mut self.chunk) {
                        Err(e) if e.kind() == ErrorKind::Interrupted => {}
                        other => break other,
                    }
                };
                self.chunk.truncate(*n.as_ref().unwrap_or(&0));
                self.at = 0;
                return n.map(|_| ());
            }
            let ReadState::Idle(file) = std::mem::replace(&mut self.state, ReadState::Broken)
            else {
                unreachable!()
            };
            let (tx, chunks) = sync_channel(DEPTH);
            let (spare, spare_rx) = sync_channel(DEPTH + 2);
            let thread = spawn(move || read_ahead(file, tx, spare_rx))?;
            self.state = ReadState::Running {
                chunks,
                spare,
                thread,
            };
        }
        let ReadState::Running { chunks, spare, .. } = &self.state else {
            return Err(worker_lost());
        };
        match chunks.recv() {
            Ok(Ok(next)) if next.is_empty() => {
                // The thread has ended at the end of the file.
                let file = self.stop()?;
                self.state = ReadState::Idle(file);
                self.at_end = true;
                Ok(())
            }
            Ok(Ok(next)) => {
                let old = std::mem::replace(&mut self.chunk, next);
                let _ = spare.try_send(old);
                self.at = 0;
                Ok(())
            }
            Ok(Err(e)) => {
                // The thread has ended; take the file back so a seek can retry.
                let file = self.stop()?;
                self.state = ReadState::Idle(file);
                Err(e)
            }
            Err(_) => {
                let _ = self.stop();
                Err(worker_lost())
            }
        }
    }
}

impl MediaSource for FileSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.at == self.chunk.len() {
            self.next_chunk()?;
        }
        let n = buf.len().min(self.chunk.len() - self.at);
        buf[..n].copy_from_slice(&self.chunk[self.at..self.at + n]);
        self.at += n;
        self.pos += n as u64;
        Ok(n)
    }

    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        if !self.is_seekable() {
            return Err(io::Error::new(
                ErrorKind::Unsupported,
                "source is not seekable",
            ));
        }
        let target = match pos {
            SeekFrom::Current(d) => SeekFrom::Start(
                self.pos
                    .checked_add_signed(d)
                    .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "seek before start"))?,
            ),
            other => other,
        };
        let mut file = self.stop()?;
        self.at_end = false;
        self.fresh = true;
        let result = Seek::seek(&mut file, target);
        if let Ok(at) = result {
            self.pos = at;
        } else {
            // Back where the caller was, as a failed seek on a file leaves it.
            Seek::seek(&mut file, SeekFrom::Start(self.pos))?;
        }
        self.state = ReadState::Idle(file);
        result
    }

    fn byte_len(&self) -> Option<u64> {
        self.byte_len
    }

    fn is_seekable(&self) -> bool {
        self.byte_len.is_some()
    }
}

impl Drop for FileSource {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// Writes on its own thread so the kernel's copy overlaps the conversion. The
/// first error ends the thread and comes back from the next call.
struct FileSink {
    buf: Vec<u8>,
    state: WriteState,
}

enum WriteState {
    Idle(File),
    Running {
        chunks: SyncSender<Vec<u8>>,
        spare: Receiver<Vec<u8>>,
        thread: JoinHandle<(File, io::Result<()>)>,
    },
    Failed,
}

fn write_behind(
    mut file: File,
    chunks: Receiver<Vec<u8>>,
    spare: SyncSender<Vec<u8>>,
) -> (File, io::Result<()>) {
    for mut buf in chunks {
        if let Err(e) = Write::write_all(&mut file, &buf) {
            return (file, Err(e));
        }
        buf.clear();
        let _ = spare.try_send(buf);
    }
    (file, Ok(()))
}

impl FileSink {
    fn new(file: File) -> Self {
        Self {
            buf: Vec::with_capacity(CHUNK),
            state: WriteState::Idle(file),
        }
    }

    fn send(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        if let WriteState::Idle(_) = self.state {
            let WriteState::Idle(file) = std::mem::replace(&mut self.state, WriteState::Failed)
            else {
                unreachable!()
            };
            let (chunks, rx) = sync_channel(DEPTH);
            let (spare_tx, spare) = sync_channel(DEPTH + 2);
            let thread = spawn(move || write_behind(file, rx, spare_tx))?;
            self.state = WriteState::Running {
                chunks,
                spare,
                thread,
            };
        }
        let WriteState::Running { chunks, spare, .. } = &self.state else {
            return Err(worker_lost());
        };
        let next = spare
            .try_recv()
            .unwrap_or_else(|_| Vec::with_capacity(CHUNK));
        let full = std::mem::replace(&mut self.buf, next);
        if chunks.send(full).is_err() {
            // The thread stopped on an error; `stop` returns it.
            self.stop()?;
            return Err(worker_lost());
        }
        Ok(())
    }

    /// Waits for everything sent so far to be written and takes the file back.
    fn stop(&mut self) -> io::Result<File> {
        match std::mem::replace(&mut self.state, WriteState::Failed) {
            WriteState::Idle(file) => Ok(file),
            WriteState::Running {
                chunks,
                spare,
                thread,
            } => {
                drop((chunks, spare));
                let (file, result) = thread.join().map_err(|_| worker_lost())?;
                result.map(|()| file)
            }
            WriteState::Failed => Err(io::Error::other("an earlier write to this file failed")),
        }
    }

    fn settle(&mut self) -> io::Result<File> {
        self.send()?;
        self.stop()
    }
}

impl MediaSink for FileSink {
    fn write_all(&mut self, mut data: &[u8]) -> io::Result<()> {
        while !data.is_empty() {
            let n = data.len().min(CHUNK - self.buf.len());
            self.buf.extend_from_slice(&data[..n]);
            data = &data[n..];
            if self.buf.len() == CHUNK {
                self.send()?;
            }
        }
        Ok(())
    }

    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let mut file = self.settle()?;
        let at = Seek::seek(&mut file, pos);
        self.state = WriteState::Idle(file);
        at
    }

    // No `sync_all`: on macOS it is `F_FULLFSYNC`, a drive-cache flush that
    // cost 7 ms on a 10 MB file, more than the conversion around it. Like `cp`
    // and `flac`, the output is left to the page cache; the rename still
    // keeps a half-written file from ever appearing under the target name.
    fn finish(&mut self) -> io::Result<()> {
        let file = self.settle()?;
        self.state = WriteState::Idle(file);
        Ok(())
    }
}

impl Drop for FileSink {
    fn drop(&mut self) {
        // As a plain file would, bytes written before a drop reach the file.
        let _ = self.settle();
    }
}

impl JobIo for FsIo {
    fn open_source(&mut self, path: &str) -> Result<Box<dyn MediaSource>, Error> {
        let file = File::open(path).map_err(|e| io_error(path, &e))?;
        Ok(Box::new(FileSource::new(file)))
    }

    fn create_sink(&mut self, path: &str) -> Result<Box<dyn MediaSink>, Error> {
        // `create_new` so a stale file from a recycled pid is never reused or
        // truncated: take the next name instead.
        let mut serial = 0;
        let (temp, file) = loop {
            let temp = temp_path(Path::new(path), serial);
            match OpenOptions::new().write(true).create_new(true).open(&temp) {
                Ok(file) => break (temp, file),
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                    serial = TEMP_SERIAL.fetch_add(1, Ordering::Relaxed) + 1;
                }
                Err(e) => return Err(io_error(path, &e)),
            }
        };
        self.temps.insert(path.to_owned(), temp);
        Ok(Box::new(FileSink::new(file)))
    }

    fn commit(&mut self, path: &str) -> Result<(), Error> {
        let temp = self.temps.remove(path).ok_or_else(|| Error::Io {
            kind: ErrorKind::NotFound,
            message: format!("{path}: no output in progress"),
        })?;
        std::fs::rename(&temp, path).map_err(|e| {
            let _ = std::fs::remove_file(&temp);
            io_error(path, &e)
        })
    }

    fn discard(&mut self, path: &str) {
        if let Some(temp) = self.temps.remove(path) {
            let _ = std::fs::remove_file(temp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FsIo;
    use crossfade_core::{Error, JobIo};
    use std::io::{ErrorKind, SeekFrom};
    use std::path::{Path, PathBuf};

    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let p =
                std::env::temp_dir().join(format!("crossfade-fsio-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }

        fn path(&self, name: &str) -> String {
            self.0.join(name).to_string_lossy().into_owned()
        }

        fn listing(&self) -> Vec<String> {
            let mut v: Vec<String> = std::fs::read_dir(&self.0)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn source_reads_an_existing_file() {
        let d = Dir::new("source");
        std::fs::write(d.path("in.bin"), b"abc").unwrap();
        let src = FsIo::new().open_source(&d.path("in.bin")).unwrap();
        assert_eq!(src.byte_len(), Some(3));
    }

    #[test]
    fn source_reads_and_seeks_through_its_buffer() {
        let d = Dir::new("seek");
        let bytes: Vec<u8> = (0..=255).cycle().take(600_000).collect();
        std::fs::write(d.path("in.bin"), &bytes).unwrap();
        let mut src = FsIo::new().open_source(&d.path("in.bin")).unwrap();
        assert!(src.is_seekable());
        let mut buf = [0u8; 10];
        assert_eq!(src.read(&mut buf).unwrap(), 10);
        assert_eq!(src.seek(SeekFrom::Current(-4)).unwrap(), 6);
        src.read(&mut buf).unwrap();
        assert_eq!(buf, bytes[6..16]);
        assert_eq!(src.seek(SeekFrom::Start(599_990)).unwrap(), 599_990);
        assert_eq!(src.read(&mut buf).unwrap(), 10);
        assert_eq!(buf, bytes[599_990..]);
        assert_eq!(src.read(&mut buf).unwrap(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn a_device_is_neither_seekable_nor_sized() {
        let src = FsIo::new().open_source("/dev/null").unwrap();
        assert!(!src.is_seekable());
        assert_eq!(src.byte_len(), None);
    }

    #[test]
    fn sink_seeks_back_over_buffered_writes() {
        let d = Dir::new("patch");
        let mut io = FsIo::new();
        let target = d.path("out.bin");
        let mut sink = io.create_sink(&target).unwrap();
        sink.write_all(b"____").unwrap();
        sink.write_all(&[7; 3000]).unwrap();
        sink.seek(SeekFrom::Start(1)).unwrap();
        sink.write_all(b"ab").unwrap();
        sink.seek(SeekFrom::End(0)).unwrap();
        sink.write_all(b"z").unwrap();
        sink.finish().unwrap();
        io.commit(&target).unwrap();
        let out = std::fs::read(&target).unwrap();
        assert_eq!(&out[..4], b"_ab_");
        assert_eq!(out.len(), 3005);
        assert_eq!(out[3004], b'z');
    }

    #[test]
    fn a_large_file_reads_back_whole_and_seeks_back_after_its_end() {
        let d = Dir::new("large");
        let bytes: Vec<u8> = (0..3_000_017u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(d.path("in.bin"), &bytes).unwrap();
        let mut src = FsIo::new().open_source(&d.path("in.bin")).unwrap();
        let mut got = Vec::new();
        let mut buf = vec![0u8; 16_384];
        loop {
            let n = src.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            got.extend_from_slice(&buf[..n]);
        }
        assert!(got == bytes);
        assert_eq!(src.read(&mut buf).unwrap(), 0);
        assert!(src.seek(SeekFrom::Current(-3_000_018)).is_err());
        assert_eq!(src.seek(SeekFrom::End(-5)).unwrap(), 3_000_012);
        assert_eq!(src.read(&mut buf).unwrap(), 5);
        assert_eq!(buf[..5], bytes[3_000_012..]);
        assert_eq!(src.seek(SeekFrom::Start(1_000_000)).unwrap(), 1_000_000);
        src.read(&mut buf[..4]).unwrap();
        assert_eq!(buf[..4], bytes[1_000_000..1_000_004]);
    }

    #[test]
    fn a_large_output_is_written_in_order_around_a_patch() {
        let d = Dir::new("bigout");
        let mut io = FsIo::new();
        let target = d.path("out.bin");
        let mut sink = io.create_sink(&target).unwrap();
        let mut want = Vec::new();
        for i in 0..700u32 {
            let piece: Vec<u8> = (0..(i % 13) * 997).map(|j| (i + j) as u8).collect();
            sink.write_all(&piece).unwrap();
            want.extend_from_slice(&piece);
        }
        sink.seek(SeekFrom::Start(2)).unwrap();
        sink.write_all(b"PATCH").unwrap();
        want[2..7].copy_from_slice(b"PATCH");
        sink.seek(SeekFrom::End(0)).unwrap();
        sink.write_all(b"tail").unwrap();
        want.extend_from_slice(b"tail");
        sink.finish().unwrap();
        drop(sink);
        io.commit(&target).unwrap();
        assert!(std::fs::read(&target).unwrap() == want);
    }

    #[test]
    fn missing_input_error_names_the_path() {
        let d = Dir::new("missing");
        let Err(Error::Io { kind, message }) = FsIo::new().open_source(&d.path("nope.wav")) else {
            panic!("expected an Io error");
        };
        assert_eq!(kind, ErrorKind::NotFound);
        assert!(message.contains("nope.wav"), "{message}");
    }

    #[test]
    fn sink_is_hidden_until_commit_then_renamed_into_place() {
        let d = Dir::new("commit");
        let mut io = FsIo::new();
        let target = d.path("out.wav");
        let mut sink = io.create_sink(&target).unwrap();
        sink.write_all(b"data").unwrap();
        sink.finish().unwrap();
        assert!(!Path::new(&target).exists());
        let names = d.listing();
        assert_eq!(names.len(), 1);
        assert!(names[0].starts_with('.'), "{names:?}");
        assert!(
            names[0].contains(&std::process::id().to_string()),
            "{names:?}"
        );
        io.commit(&target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"data");
        assert_eq!(d.listing(), ["out.wav"]);
    }

    #[test]
    fn two_writers_to_one_target_never_share_a_temp_file() {
        let d = Dir::new("collide");
        let target = d.path("out.wav");
        let (mut a, mut b) = (FsIo::new(), FsIo::new());
        a.create_sink(&target).unwrap().write_all(b"aaaa").unwrap();
        b.create_sink(&target).unwrap().write_all(b"b").unwrap();
        assert_eq!(d.listing().len(), 2, "{:?}", d.listing());
        b.commit(&target).unwrap();
        a.discard(&target);
        assert_eq!(std::fs::read(&target).unwrap(), b"b");
        assert_eq!(d.listing(), ["out.wav"]);
    }

    #[test]
    fn commit_replaces_an_existing_file() {
        let d = Dir::new("replace");
        let target = d.path("out.wav");
        std::fs::write(&target, b"old").unwrap();
        let mut io = FsIo::new();
        io.create_sink(&target).unwrap().write_all(b"new").unwrap();
        io.commit(&target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
    }

    #[test]
    fn discard_leaves_the_directory_as_it_was() {
        let d = Dir::new("discard");
        let target = d.path("out.wav");
        std::fs::write(&target, b"keep").unwrap();
        let mut io = FsIo::new();
        io.create_sink(&target)
            .unwrap()
            .write_all(b"partial")
            .unwrap();
        io.discard(&target);
        assert_eq!(d.listing(), ["out.wav"]);
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
        io.discard(&target);
    }

    #[test]
    fn commit_without_a_sink_is_an_error() {
        let d = Dir::new("nosink");
        assert!(matches!(
            FsIo::new().commit(&d.path("x.wav")),
            Err(Error::Io { .. })
        ));
    }

    #[test]
    fn unwritable_directory_is_an_io_error_naming_the_path() {
        let d = Dir::new("nodir");
        let target = d.path("missing-dir/out.wav");
        let Err(Error::Io { message, .. }) = FsIo::new().create_sink(&target) else {
            panic!("expected an Io error");
        };
        assert!(message.contains("out.wav"), "{message}");
    }
}
