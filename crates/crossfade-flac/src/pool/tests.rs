use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Work whose cost varies with the item, so workers finish out of order.
fn churn(_: &mut (), x: u64) -> u64 {
    let mut h = x;
    for _ in 0..(x % 7) * 2000 {
        h = h.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    }
    std::hint::black_box(h);
    x * 3
}

fn pool(workers: usize, depth: usize) -> Ordered<u64, u64, ()> {
    Ordered::new(workers, depth, || (), churn)
}

/// Feeds `items` in, never letting more than the capacity be in flight, and collects the
/// results in the order they come out.
fn drive(p: &mut Ordered<u64, u64, ()>, items: impl IntoIterator<Item = u64>) -> Vec<u64> {
    let mut out = Vec::new();
    for x in items {
        while p.is_full() {
            out.push(p.next(&mut ()).unwrap().unwrap());
        }
        p.submit(x);
        while let Some(r) = p.try_next() {
            out.push(r.unwrap());
        }
    }
    while let Some(r) = p.next(&mut ()) {
        out.push(r.unwrap());
    }
    out
}

#[test]
fn results_come_out_in_the_order_items_went_in() {
    for workers in [0, 1, 2, 3, 8] {
        for depth in [1, 2, 4] {
            let mut p = pool(workers, depth);
            let got = drive(&mut p, 0..500);
            let want: Vec<u64> = (0..500).map(|x| x * 3).collect();
            assert_eq!(got, want, "{workers} workers, depth {depth}");
            assert!(p.workers() <= workers);
            assert_eq!(p.in_flight(), 0);
        }
    }
}

#[test]
fn in_flight_is_bounded_by_the_capacity() {
    let mut p = pool(3, 2);
    assert_eq!(p.capacity(), (3 + 1) * 2);
    for x in 0..p.capacity() as u64 {
        assert!(!p.is_full());
        p.submit(x);
    }
    assert!(p.is_full());
    assert_eq!(p.in_flight(), p.capacity());
    assert_eq!(p.next(&mut ()), Some(Ok(0)));
    assert!(!p.is_full());
}

#[test]
fn workers_start_only_when_items_queue_up() {
    let mut p = pool(8, 2);
    p.submit(1);
    // One item: the caller runs it itself when it asks for the result.
    assert_eq!(p.workers(), 0);
    assert_eq!(p.next(&mut ()), Some(Ok(3)));
    assert_eq!(p.workers(), 0);
    assert_eq!(p.next(&mut ()), None);
    p.submit(2);
    p.submit(3);
    assert_eq!(p.workers(), 1);
    assert_eq!(drive(&mut p, []), [6, 9]);
}

#[test]
fn no_workers_means_the_caller_does_everything() {
    let mut p = pool(0, 2);
    assert_eq!(drive(&mut p, 0..20).len(), 20);
    assert_eq!(p.workers(), 0);
}

/// Each worker's state counts the items it ran; the caller's is separate.
#[test]
fn every_worker_keeps_its_own_state() {
    let made = Arc::new(AtomicUsize::new(0));
    let m = Arc::clone(&made);
    let mut p: Ordered<u64, (u64, usize), usize> = Ordered::new(
        4,
        2,
        move || {
            m.fetch_add(1, Ordering::SeqCst);
            0
        },
        |seen: &mut usize, x| {
            *seen += 1;
            (x, *seen)
        },
    );
    let mut local = 0usize;
    let mut total = 0;
    for x in 0..200 {
        while p.is_full() {
            total += 1;
            assert_eq!(p.next(&mut local).unwrap().unwrap().0, total - 1);
        }
        p.submit(x);
    }
    while let Some(r) = p.next(&mut local) {
        total += 1;
        assert_eq!(r.unwrap().0, total - 1);
    }
    assert_eq!(total, 200);
    let workers = p.workers();
    // Each worker builds its state on its own thread; joined, they all have.
    drop(p);
    assert_eq!(made.load(Ordering::SeqCst), workers);
}

fn picky(_: &mut (), x: u64) -> u64 {
    assert!(x != 5, "item five");
    x
}

#[test]
fn a_panic_in_a_worker_comes_out_as_its_result() {
    for workers in [0, 1, 4] {
        let mut p: Ordered<u64, u64, ()> = Ordered::new(workers, 2, || (), picky);
        let mut got = Vec::new();
        for x in 0..12 {
            while p.is_full() {
                got.push(p.next(&mut ()).unwrap());
            }
            p.submit(x);
        }
        while let Some(r) = p.next(&mut ()) {
            got.push(r);
        }
        assert_eq!(got.len(), 12, "{workers} workers");
        for (x, r) in got.iter().enumerate() {
            if x == 5 {
                assert_eq!(r, &Err(Panicked("item five".into())), "{workers} workers");
            } else {
                assert_eq!(r, &Ok(x as u64), "{workers} workers");
            }
        }
    }
}

/// Dropping the pool with work queued and never read neither hangs nor leaves a thread: every
/// worker's state is dropped by the time `drop` returns.
#[test]
fn dropping_a_full_unread_pool_joins_every_worker() {
    let alive = Arc::new(());
    let a = Arc::clone(&alive);
    let mut p: Ordered<u64, u64, Arc<()>> = Ordered::new(6, 3, move || Arc::clone(&a), |_, x| x);
    while !p.is_full() {
        p.submit(1);
    }
    assert!(p.workers() >= 1);
    drop(p);
    assert_eq!(Arc::strong_count(&alive), 1);
}

/// A worker blocked inside an item: closing lets it finish that item and no other.
#[test]
fn a_closed_pool_runs_nothing_more_than_the_items_already_started() {
    struct Gate {
        started: Sender<()>,
        open: Arc<Mutex<Receiver<()>>>,
        runs: Arc<AtomicUsize>,
    }
    fn wait(g: &mut Gate, x: u64) -> u64 {
        g.runs.fetch_add(1, Ordering::SeqCst);
        g.started.send(()).unwrap();
        g.open.lock().unwrap().recv().unwrap();
        x
    }
    let (started, on_start) = channel();
    let (opener, open) = channel();
    let open = Arc::new(Mutex::new(open));
    let runs = Arc::new(AtomicUsize::new(0));
    let (s, o, r) = (started.clone(), Arc::clone(&open), Arc::clone(&runs));
    let mut p: Ordered<u64, u64, Gate> = Ordered::new(
        1,
        8,
        move || Gate {
            started: s.clone(),
            open: Arc::clone(&o),
            runs: Arc::clone(&r),
        },
        wait,
    );
    for x in 0..6 {
        p.submit(x);
    }
    assert_eq!(p.workers(), 1);
    on_start.recv().unwrap();
    p.close();
    opener.send(()).unwrap();
    drop(p);
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    drop(started);
}

#[allow(clippy::ptr_arg)] // the stage passes a reference to its item type
fn record(seen: &mut Vec<u64>, x: &Vec<u64>) {
    seen.extend_from_slice(x);
}

#[test]
fn a_serial_stage_sees_every_item_in_order_and_hands_buffers_back() {
    let mut stage = Serial::new(Vec::new(), record, 2).unwrap();
    let mut reused = 0;
    for x in 0..300u64 {
        let mut buf = stage.spare().unwrap_or_default();
        if buf.capacity() > 0 {
            reused += 1;
        }
        buf.clear();
        buf.extend([x, x + 1000]);
        stage.send(buf).unwrap();
    }
    let seen = stage.finish().unwrap();
    let want: Vec<u64> = (0..300).flat_map(|x| [x, x + 1000]).collect();
    assert_eq!(seen, want);
    assert!(reused > 0, "no buffer came back");
}

fn fussy(_: &mut (), x: &u64) {
    assert!(*x != 3, "three");
}

#[test]
fn a_panic_in_a_serial_stage_is_an_error_not_a_hang() {
    let mut stage = Serial::new((), fussy, 1).unwrap();
    let mut failed = false;
    for x in 0..100 {
        if stage.send(x).is_err() {
            failed = true;
            break;
        }
    }
    let end = stage.finish();
    assert!(failed || end.is_err());
    assert_eq!(end, Err(Panicked("three".into())));
}

#[test]
fn dropping_a_serial_stage_joins_its_thread() {
    let alive = Arc::new(());
    let mut stage = Serial::new(Arc::clone(&alive), |_: &mut Arc<()>, _: &u64| {}, 4).unwrap();
    for x in 0..10 {
        stage.send(x).unwrap();
    }
    drop(stage);
    assert_eq!(Arc::strong_count(&alive), 1);
}
