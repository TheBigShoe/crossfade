//! The waveform strip's lifecycle: which file is shown, its analysis (running, finished or not
//! asked for), the cache of finished ones, and the coalescing of the worker's wake-ups.

use crate::analysis::{Analyzer, State};
use crate::waveview::WaveView;
use std::sync::atomic::{AtomicBool, Ordering};

/// Heap held by cached analyses, at most.
pub const CACHE_CAP: usize = 256 * 1024 * 1024;

/// Entries by key under a byte cap, oldest first; the oldest go when the cap is passed.
#[derive(Debug)]
pub struct Cache<T> {
    cap: usize,
    items: Vec<(String, usize, T)>,
}

impl<T> Cache<T> {
    pub fn new(cap: usize) -> Self {
        Self {
            cap,
            items: Vec::new(),
        }
    }

    pub fn bytes(&self) -> usize {
        self.items.iter().map(|(_, b, _)| b).sum()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Oldest first.
    pub fn keys(&self) -> Vec<&str> {
        self.items.iter().map(|(k, _, _)| k.as_str()).collect()
    }

    pub fn contains(&self, key: &str) -> bool {
        self.items.iter().any(|(k, _, _)| k == key)
    }

    /// Adds the newest entry. One that alone passes the cap is not kept (and evicts nothing).
    pub fn insert(&mut self, key: String, bytes: usize, value: T) {
        self.items.retain(|(k, _, _)| *k != key);
        if bytes > self.cap {
            return;
        }
        self.items.push((key, bytes, value));
        while self.bytes() > self.cap {
            self.items.remove(0);
        }
    }

    pub fn take(&mut self, key: &str) -> Option<T> {
        let at = self.items.iter().position(|(k, _, _)| k == key)?;
        Some(self.items.remove(at).2)
    }
}

/// Lets a worker thread ask for one redraw at a time: further requests are absorbed until the
/// UI has looked, so a long analysis cannot flood it.
#[derive(Debug, Default)]
pub struct Wake(AtomicBool);

impl Wake {
    /// True when the caller should send the wake-up.
    pub fn request(&self) -> bool {
        !self.0.swap(true, Ordering::AcqRel)
    }

    /// The UI is about to read the latest state.
    pub fn acknowledge(&self) {
        self.0.store(false, Ordering::Release);
    }
}

pub struct Strip {
    current: Option<(String, Option<Analyzer>)>,
    cache: Cache<Analyzer>,
    view: WaveView,
}

impl Default for Strip {
    fn default() -> Self {
        Self::new(CACHE_CAP)
    }
}

impl Strip {
    pub fn new(cache_cap: usize) -> Self {
        Self {
            current: None,
            cache: Cache::new(cache_cap),
            view: WaveView::new(),
        }
    }

    pub fn path(&self) -> Option<&str> {
        self.current.as_ref().map(|(p, _)| p.as_str())
    }

    /// The analysis of the shown file; `None` when nothing is shown or none was started.
    pub fn analyzer(&self) -> Option<&Analyzer> {
        self.current.as_ref().and_then(|(_, a)| a.as_ref())
    }

    pub fn is_cached(&self, path: &str) -> bool {
        self.cache.contains(path)
    }

    pub fn view(&self) -> &WaveView {
        &self.view
    }

    pub fn view_mut(&mut self) -> &mut WaveView {
        &mut self.view
    }

    /// Show `path` (or nothing). A finished analysis of the file left behind is kept, a running
    /// one is cancelled; the new file's comes from the cache, from `start` when `auto`, or is
    /// left for [`analyse`](Self::analyse). Selecting the file already shown does nothing.
    pub fn select(&mut self, path: Option<&str>, auto: bool, start: impl FnOnce(&str) -> Analyzer) {
        if self.path() == path {
            return;
        }
        if let Some((old, Some(analyzer))) = self.current.take() {
            // Dropping a running one cancels it.
            if analyzer.snapshot().state == State::Done {
                let bytes = analyzer.with(|a| a.pyramid.heap_bytes());
                self.cache.insert(old, bytes, analyzer);
            }
        }
        let width = self.view.width();
        self.view = WaveView::new();
        self.view.set_width(width);
        let Some(path) = path else {
            return;
        };
        let analyzer = match self.cache.take(path) {
            Some(cached) => Some(cached),
            None if auto => Some(start(path)),
            None => None,
        };
        self.current = Some((path.to_owned(), analyzer));
    }

    /// Start the shown file's analysis when automatic analysis is off.
    pub fn analyse(&mut self, start: impl FnOnce(&str) -> Analyzer) {
        if let Some((path, analyzer @ None)) = &mut self.current {
            *analyzer = Some(start(path));
        }
    }
}
