//! `JobIo` over the real file system.

use crossfade_core::{Error, JobIo, MediaSink, MediaSource};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, BufWriter, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

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

/// Readers ask for a packet at a time (16 KiB for CD audio); one system call
/// per request costs more than the copy out of a larger buffer.
const READ_BUFFER: usize = 256 * 1024;
/// Writes go out in large blocks for the same reason.
const WRITE_BUFFER: usize = 1024 * 1024;

/// A buffered file whose kind is looked up once, not on every question.
struct FileSource {
    file: BufReader<File>,
    byte_len: Option<u64>,
}

impl FileSource {
    fn new(file: File) -> Self {
        // Devices and pipes opened as `File` report no length and cannot seek.
        let byte_len = file
            .metadata()
            .ok()
            .filter(std::fs::Metadata::is_file)
            .map(|m| m.len());
        Self {
            file: BufReader::with_capacity(READ_BUFFER, file),
            byte_len,
        }
    }
}

impl MediaSource for FileSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.file.read(buf)
    }

    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.file.seek(pos)
    }

    fn byte_len(&self) -> Option<u64> {
        self.byte_len
    }

    fn is_seekable(&self) -> bool {
        self.byte_len.is_some()
    }
}

struct FileSink(BufWriter<File>);

impl MediaSink for FileSink {
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.0.write_all(buf)
    }

    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.0.seek(pos)
    }

    fn finish(&mut self) -> io::Result<()> {
        self.0.flush()?;
        self.0.get_ref().sync_all()
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
        Ok(Box::new(FileSink(BufWriter::with_capacity(WRITE_BUFFER, file))))
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
