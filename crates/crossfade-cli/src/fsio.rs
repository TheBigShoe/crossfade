//! `JobIo` over the real file system.

use crossfade_core::{Error, JobIo, MediaSink, MediaSource};
use std::collections::HashMap;
use std::fs::File;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// Output goes to a hidden temporary file beside the target; `commit` renames
/// it into place, so a reader never sees a half-written file.
#[derive(Default)]
pub(crate) struct FsIo {
    temps: HashMap<String, PathBuf>,
}

fn io_error(path: &str, e: &std::io::Error) -> Error {
    Error::Io {
        kind: e.kind(),
        message: format!("{path}: {e}"),
    }
}

fn temp_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map_or_else(|| "output".into(), |n| n.to_string_lossy());
    let temp = format!(".{name}.{}.tmp", std::process::id());
    match target.parent() {
        Some(dir) => dir.join(temp),
        None => PathBuf::from(temp),
    }
}

impl FsIo {
    pub(crate) fn new() -> Self {
        Self::default()
    }
}

impl JobIo for FsIo {
    fn open_source(&mut self, path: &str) -> Result<Box<dyn MediaSource>, Error> {
        let file = File::open(path).map_err(|e| io_error(path, &e))?;
        Ok(Box::new(file))
    }

    fn create_sink(&mut self, path: &str) -> Result<Box<dyn MediaSink>, Error> {
        let temp = temp_path(Path::new(path));
        let file = File::create(&temp).map_err(|e| io_error(path, &e))?;
        self.temps.insert(path.to_owned(), temp);
        Ok(Box::new(file))
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
    use std::io::ErrorKind;
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
