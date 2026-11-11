//! Threads for the codecs, on `std` alone: an ordered worker pool and a serial stage.
//!
//! [`Ordered`] takes items in order, runs them on worker threads that each keep their own state,
//! and gives the results back in the order the items went in. The caller is the producer and
//! the consumer: it may have at most [`capacity`](Ordered::capacity) items in flight, so when
//! the pool [`is_full`](Ordered::is_full) it takes a result before submitting more (that is the
//! back-pressure, and it bounds memory). While it waits for a result it runs queued items
//! itself with its own state, so it is never idle while there is work and a pool with no
//! workers is just a queue. Workers start lazily, one each time items queue up with nobody free
//! to take them, so a stream of a few items starts few threads or none.
//!
//! [`Serial`] runs one fold over items in order on a thread of its own (a stream's MD5),
//! handing each item back for reuse.
//!
//! Nothing here busy-waits or sleeps: threads block on a condition variable or a channel. A
//! panic in an item comes back as that item's [`Panicked`] result; dropping either type stops
//! and joins its threads, letting a worker finish only the item it is on.

use std::any::Any;
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{Builder, JoinHandle};

/// An item's work panicked; the panic's message, when it had one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Panicked(pub(crate) String);

impl Panicked {
    fn from_payload(payload: &(dyn Any + Send)) -> Self {
        let text = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "a worker thread panicked".into());
        Self(text)
    }
}

const THREAD_NAME: &str = "crossfade-flac";

enum Slot<O> {
    Waiting,
    Done(Result<O, Panicked>),
}

struct State<I, O> {
    /// Items no thread has taken yet, with their sequence numbers.
    queue: VecDeque<(u64, I)>,
    /// One slot per item in flight, from sequence number `first` on.
    slots: VecDeque<Slot<O>>,
    first: u64,
    /// Workers waiting for an item.
    idle: usize,
    closed: bool,
}

impl<I, O> State<I, O> {
    fn store(&mut self, seq: u64, result: Result<O, Panicked>) {
        // A closed pool has dropped its slots; the result goes with them.
        if let Some(slot) = seq
            .checked_sub(self.first)
            .and_then(|i| self.slots.get_mut(i as usize))
        {
            *slot = Slot::Done(result);
        }
    }
}

struct Shared<I, O> {
    state: Mutex<State<I, O>>,
    /// Workers wait here for items.
    work: Condvar,
    /// The caller waits here for a result.
    done: Condvar,
}

impl<I, O> Shared<I, O> {
    // A panic never happens while the lock is held (items run outside it), so a poisoned lock
    // still guards consistent state.
    fn lock(&self) -> MutexGuard<'_, State<I, O>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn close(&self) {
        let mut state = self.lock();
        state.closed = true;
        let queue = std::mem::take(&mut state.queue);
        let slots = std::mem::take(&mut state.slots);
        self.work.notify_all();
        self.done.notify_all();
        drop(state);
        // Items and results may own memory worth freeing outside the lock.
        drop((queue, slots));
    }
}

type Make<W> = Arc<dyn Fn() -> W + Send + Sync>;

/// Runs `run` and turns a panic into an error. After one the caller rebuilds the state (the
/// panic may have left it half-updated), once the result is stored: should rebuilding panic
/// too, nobody is left waiting for that result.
fn guarded<I, O, W>(run: fn(&mut W, I) -> O, state: &mut W, item: I) -> Result<O, Panicked> {
    catch_unwind(AssertUnwindSafe(|| run(state, item)))
        .map_err(|payload| Panicked::from_payload(&*payload))
}

/// An ordered worker pool; see the module documentation.
pub(crate) struct Ordered<I, O, W> {
    shared: Arc<Shared<I, O>>,
    make: Make<W>,
    run: fn(&mut W, I) -> O,
    handles: Vec<JoinHandle<()>>,
    max_workers: usize,
    capacity: usize,
    /// Sequence numbers of the next item in and the next result out.
    next_in: u64,
    next_out: u64,
}

impl<I: Send + 'static, O: Send + 'static, W: 'static> Ordered<I, O, W> {
    /// Up to `workers` threads, each with state from `make` (built on that thread), running
    /// `run`; `depth` items in flight for each thread that works, the caller included.
    pub(crate) fn new(
        workers: usize,
        depth: usize,
        make: impl Fn() -> W + Send + Sync + 'static,
        run: fn(&mut W, I) -> O,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    queue: VecDeque::new(),
                    slots: VecDeque::new(),
                    first: 0,
                    idle: 0,
                    closed: false,
                }),
                work: Condvar::new(),
                done: Condvar::new(),
            }),
            make: Arc::new(make),
            run,
            handles: Vec::new(),
            max_workers: workers,
            capacity: (workers + 1) * depth.max(1),
            next_in: 0,
            next_out: 0,
        }
    }

    /// How many items may be in flight at once.
    #[cfg(test)]
    pub(crate) fn capacity(&self) -> usize {
        self.capacity
    }

    /// Items submitted whose results have not been taken.
    pub(crate) fn in_flight(&self) -> usize {
        (self.next_in - self.next_out) as usize
    }

    /// Take a result before submitting more.
    pub(crate) fn is_full(&self) -> bool {
        self.in_flight() >= self.capacity
    }

    /// Worker threads started so far.
    pub(crate) fn workers(&self) -> usize {
        self.handles.len()
    }

    /// Queues `item`. The caller keeps the pool from overfilling: when it [`is_full`]
    /// (Self::is_full), take a result first.
    pub(crate) fn submit(&mut self, item: I) {
        debug_assert!(!self.is_full(), "submit to a full pool");
        let mut state = self.shared.lock();
        state.queue.push_back((self.next_in, item));
        state.slots.push_back(Slot::Waiting);
        self.next_in += 1;
        // One item waiting is left for the caller, which runs it when it wants the result;
        // a second with nobody free to take it is worth a thread.
        let spawn = state.queue.len() >= state.idle + 2 && self.handles.len() < self.max_workers;
        if state.idle > 0 {
            self.shared.work.notify_one();
        }
        drop(state);
        if spawn {
            self.spawn();
        }
    }

    fn spawn(&mut self) {
        let shared = Arc::clone(&self.shared);
        let make = Arc::clone(&self.make);
        let run = self.run;
        let started = Builder::new()
            .name(THREAD_NAME.into())
            .spawn(move || work(&shared, &make, run));
        match started {
            Ok(handle) => self.handles.push(handle),
            // No more threads to be had (or none at all on this target): the caller runs
            // what the workers cannot.
            Err(_) => self.max_workers = self.handles.len(),
        }
    }

    /// The next result in order if it is ready.
    pub(crate) fn try_next(&mut self) -> Option<Result<O, Panicked>> {
        if self.in_flight() == 0 {
            return None;
        }
        let shared = Arc::clone(&self.shared);
        let mut state = shared.lock();
        self.take_ready(&mut state)
    }

    fn take_ready(&mut self, state: &mut State<I, O>) -> Option<Result<O, Panicked>> {
        if !matches!(state.slots.front(), Some(Slot::Done(_))) {
            return None;
        }
        let Some(Slot::Done(result)) = state.slots.pop_front() else {
            unreachable!("checked above")
        };
        state.first += 1;
        self.next_out += 1;
        Some(result)
    }

    /// The next result in order, waiting for it; `None` when nothing is in flight. While the
    /// result is not ready the caller runs queued items itself, with `local` as its state.
    pub(crate) fn next(&mut self, local: &mut W) -> Option<Result<O, Panicked>> {
        if self.in_flight() == 0 {
            return None;
        }
        let shared = Arc::clone(&self.shared);
        let mut state = shared.lock();
        loop {
            if state.closed {
                return None;
            }
            if let Some(result) = self.take_ready(&mut state) {
                return Some(result);
            }
            if let Some((seq, item)) = state.queue.pop_front() {
                drop(state);
                let result = guarded(self.run, local, item);
                let failed = result.is_err();
                state = shared.lock();
                state.store(seq, result);
                if failed {
                    drop(state);
                    *local = (self.make)();
                    state = shared.lock();
                }
            } else {
                state = shared
                    .done
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        }
    }

    /// Drops every queued item and tells the workers to stop once they finish the item they
    /// are on. Dropping the pool closes it and joins the workers.
    #[cfg(test)]
    pub(crate) fn close(&self) {
        self.shared.close();
    }
}

impl<I, O, W> Drop for Ordered<I, O, W> {
    fn drop(&mut self) {
        self.shared.close();
        for handle in self.handles.drain(..) {
            // A worker's own panics are caught per item; nothing is left to report.
            let _ = handle.join();
        }
    }
}

fn work<I, O, W>(shared: &Shared<I, O>, make: &Make<W>, run: fn(&mut W, I) -> O) {
    let mut local = make();
    let mut state = shared.lock();
    loop {
        if state.closed {
            return;
        }
        if let Some((seq, item)) = state.queue.pop_front() {
            drop(state);
            let result = guarded(run, &mut local, item);
            let failed = result.is_err();
            state = shared.lock();
            state.store(seq, result);
            shared.done.notify_one();
            if failed {
                drop(state);
                local = make();
                state = shared.lock();
            }
        } else {
            state.idle += 1;
            state = shared
                .work
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
            state.idle -= 1;
        }
    }
}

/// A fold over items in order on a thread of its own; see the module documentation.
pub(crate) struct Serial<S, T> {
    items: Option<SyncSender<T>>,
    spares: Receiver<T>,
    handle: Option<JoinHandle<Option<S>>>,
}

impl<S: Send + 'static, T: Send + 'static> Serial<S, T> {
    /// Starts the thread with `state`; up to `depth` items wait for it before
    /// [`send`](Self::send) blocks. When no thread can be started, the state comes back.
    pub(crate) fn new(state: S, run: fn(&mut S, &T), depth: usize) -> Result<Self, S> {
        let (items, inbox) = sync_channel::<T>(depth);
        let (back, spares) = channel();
        // The state goes over once the thread exists, so a failed spawn can hand it back.
        let (give, take) = channel::<S>();
        let started = Builder::new().name(THREAD_NAME.into()).spawn(move || {
            let state = take.recv().ok()?;
            Some(fold(state, run, &inbox, &back))
        });
        let Ok(handle) = started else {
            return Err(state);
        };
        // The thread holds `take` until it has the state, so this cannot fail.
        let _ = give.send(state);
        Ok(Self {
            items: Some(items),
            spares,
            handle: Some(handle),
        })
    }

    /// Hands `item` to the thread, waiting while `depth` items are ahead of it. An error means
    /// the thread panicked; [`finish`](Self::finish) says with what.
    pub(crate) fn send(&mut self, item: T) -> Result<(), Panicked> {
        let sent = self.items.as_ref().map(|items| items.send(item));
        match sent {
            Some(Ok(())) => Ok(()),
            _ => Err(Panicked("the serial stage stopped".into())),
        }
    }

    /// An item the thread is done with, for reuse.
    pub(crate) fn spare(&mut self) -> Option<T> {
        self.spares.try_recv().ok()
    }

    /// Waits for every item sent to be folded in, and returns the state.
    pub(crate) fn finish(mut self) -> Result<S, Panicked> {
        self.items = None;
        let handle = self.handle.take().expect("only finish takes the handle");
        match handle.join() {
            Ok(Some(state)) => Ok(state),
            Ok(None) => Err(Panicked("the serial stage never started".into())),
            Err(payload) => Err(Panicked::from_payload(&*payload)),
        }
    }
}

fn fold<S, T>(mut state: S, run: fn(&mut S, &T), inbox: &Receiver<T>, back: &Sender<T>) -> S {
    while let Ok(item) = inbox.recv() {
        run(&mut state, &item);
        // The sender may be gone already; then the item is simply dropped.
        let _ = back.send(item);
    }
    state
}

impl<S, T> Drop for Serial<S, T> {
    fn drop(&mut self) {
        // Closing the channel ends the thread's loop once it has the items already sent.
        self.items = None;
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests;
