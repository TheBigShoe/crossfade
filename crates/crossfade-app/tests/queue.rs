use crossfade_app::Request;
use crossfade_app::queue::{JobId, JobState, Queue, Runner, RunnerEvent, Summary};
use crossfade_graph::{AudioSettings, CancelToken, Job, JobInput, JobOutput};
use proptest::prelude::*;

fn request(name: &str) -> Request {
    Request {
        job: Job {
            version: Job::VERSION,
            inputs: vec![JobInput {
                path: format!("{name}.wav"),
            }],
            outputs: vec![JobOutput {
                path: format!("{name}-out.wav"),
                audio: AudioSettings::default(),
            }],
        },
        overwrite: false,
    }
}

/// Records what the queue asked for; the test plays the runner's part.
#[derive(Default)]
struct Scripted {
    started: Vec<(JobId, Request, CancelToken)>,
}

impl Runner for Scripted {
    fn start(&mut self, id: JobId, request: &Request, cancel: CancelToken) {
        self.started.push((id, request.clone(), cancel));
    }
}

/// An id the queue once issued and no longer holds.
fn gone(q: &mut Queue) -> JobId {
    let id = q.enqueue(request("gone"));
    assert!(q.remove(id));
    id
}

fn state(q: &Queue, id: JobId) -> JobState {
    q.get(id).unwrap().state.clone()
}

fn running(done: u64, total: Option<u64>, cancelling: bool) -> JobState {
    JobState::Running {
        frames_done: done,
        frames_total: total,
        cancelling,
    }
}

fn progress(done: u64, total: Option<u64>) -> RunnerEvent {
    RunnerEvent::Progress {
        frames_done: done,
        frames_total: total,
    }
}

#[test]
fn a_job_runs_from_pending_to_finished() {
    let (mut q, mut r) = (Queue::new(), Scripted::default());
    let id = q.enqueue(request("a"));
    assert_eq!(state(&q, id), JobState::Pending);
    q.pump(&mut r);
    assert_eq!(r.started.len(), 1);
    assert_eq!(r.started[0].0, id);
    assert_eq!(r.started[0].1, request("a"));
    assert_eq!(state(&q, id), running(0, None, false));
    q.apply(id, RunnerEvent::Started);
    q.apply(id, progress(50, Some(100)));
    assert_eq!(state(&q, id), running(50, Some(100), false));
    q.apply(id, RunnerEvent::Finished { frames: 100 });
    assert_eq!(state(&q, id), JobState::Finished { frames: 100 });
}

#[test]
fn jobs_start_one_at_a_time_in_order_and_only_when_pumped() {
    let (mut q, mut r) = (Queue::new(), Scripted::default());
    let ids: Vec<_> = ["a", "b", "c"]
        .iter()
        .map(|n| q.enqueue(request(n)))
        .collect();
    q.pump(&mut r);
    q.pump(&mut r);
    assert_eq!(r.started.len(), 1);
    q.apply(ids[0], RunnerEvent::Finished { frames: 1 });
    assert_eq!(r.started.len(), 1, "a terminal event must not auto-start");
    q.pump(&mut r);
    assert_eq!(r.started.iter().map(|s| s.0).collect::<Vec<_>>(), ids[..2]);
    assert_eq!(state(&q, ids[2]), JobState::Pending);
}

#[test]
fn pumping_an_empty_queue_does_nothing() {
    let (mut q, mut r) = (Queue::new(), Scripted::default());
    q.pump(&mut r);
    assert!(r.started.is_empty());
}

#[test]
fn cancelling_a_pending_job_is_immediate_and_it_never_starts() {
    let (mut q, mut r) = (Queue::new(), Scripted::default());
    let a = q.enqueue(request("a"));
    let b = q.enqueue(request("b"));
    q.cancel(a);
    assert_eq!(state(&q, a), JobState::Cancelled);
    q.pump(&mut r);
    assert_eq!(r.started[0].0, b);
}

#[test]
fn cancelling_a_running_job_waits_for_the_runner() {
    let (mut q, mut r) = (Queue::new(), Scripted::default());
    let id = q.enqueue(request("a"));
    q.pump(&mut r);
    assert!(!r.started[0].2.is_cancelled());
    q.apply(id, progress(10, Some(100)));
    q.cancel(id);
    assert!(r.started[0].2.is_cancelled());
    assert_eq!(state(&q, id), running(10, Some(100), true));
    q.apply(id, progress(20, Some(100)));
    assert_eq!(state(&q, id), running(20, Some(100), true));
    q.apply(id, RunnerEvent::Cancelled);
    assert_eq!(state(&q, id), JobState::Cancelled);
}

#[test]
fn cancelling_a_finished_job_changes_nothing() {
    let (mut q, mut r) = (Queue::new(), Scripted::default());
    let id = q.enqueue(request("a"));
    q.pump(&mut r);
    q.apply(id, RunnerEvent::Finished { frames: 3 });
    q.cancel(id);
    assert_eq!(state(&q, id), JobState::Finished { frames: 3 });
}

#[test]
fn a_failed_job_can_be_retried_at_the_back() {
    let (mut q, mut r) = (Queue::new(), Scripted::default());
    let a = q.enqueue(request("a"));
    let b = q.enqueue(request("b"));
    q.pump(&mut r);
    q.apply(
        a,
        RunnerEvent::Failed {
            message: "boom".into(),
        },
    );
    assert_eq!(
        state(&q, a),
        JobState::Failed {
            message: "boom".into()
        }
    );
    let again = q.retry(a).unwrap();
    assert_ne!(again, a);
    assert_eq!(q.get(again).unwrap().request, request("a"));
    assert_eq!(state(&q, again), JobState::Pending);
    let order: Vec<_> = q.items().iter().map(|i| i.id).collect();
    assert_eq!(order, [a, b, again]);
}

#[test]
fn only_failed_or_cancelled_jobs_retry() {
    let (mut q, mut r) = (Queue::new(), Scripted::default());
    let gone = gone(&mut q);
    let a = q.enqueue(request("a"));
    let b = q.enqueue(request("b"));
    assert_eq!(q.retry(a), None);
    q.pump(&mut r);
    assert_eq!(q.retry(a), None);
    q.apply(a, RunnerEvent::Finished { frames: 1 });
    assert_eq!(q.retry(a), None);
    q.cancel(b);
    assert!(q.retry(b).is_some());
    assert_eq!(q.retry(gone), None);
}

#[test]
fn stray_and_late_events_are_ignored() {
    let (mut q, mut r) = (Queue::new(), Scripted::default());
    let gone = gone(&mut q);
    let a = q.enqueue(request("a"));
    let b = q.enqueue(request("b"));
    q.apply(b, progress(5, None));
    q.apply(b, RunnerEvent::Finished { frames: 5 });
    assert_eq!(state(&q, b), JobState::Pending);
    q.pump(&mut r);
    q.apply(
        a,
        RunnerEvent::Failed {
            message: "x".into(),
        },
    );
    q.apply(a, RunnerEvent::Finished { frames: 1 });
    q.apply(a, progress(9, Some(9)));
    q.apply(a, RunnerEvent::Cancelled);
    assert_eq!(
        state(&q, a),
        JobState::Failed {
            message: "x".into()
        }
    );
    q.apply(gone, RunnerEvent::Cancelled);
    assert_eq!(q.items().len(), 2);
}

#[test]
fn remove_refuses_a_running_job_and_clear_done_drops_only_finished() {
    let (mut q, mut r) = (Queue::new(), Scripted::default());
    let ids: Vec<_> = ["a", "b", "c", "d"]
        .iter()
        .map(|n| q.enqueue(request(n)))
        .collect();
    q.pump(&mut r);
    assert!(!q.remove(ids[0]));
    q.apply(ids[0], RunnerEvent::Finished { frames: 1 });
    q.pump(&mut r);
    q.apply(
        ids[1],
        RunnerEvent::Failed {
            message: "x".into(),
        },
    );
    q.cancel(ids[3]);
    q.clear_done();
    let left: Vec<_> = q.items().iter().map(|i| i.id).collect();
    assert_eq!(left, [ids[1], ids[2], ids[3]]);
    assert!(q.remove(ids[1]));
    assert!(q.remove(ids[2]));
    assert!(!q.remove(ids[2]));
    assert!(q.remove(ids[3]));
    assert!(q.items().is_empty());
}

#[test]
fn fraction_is_clamped_and_absent_without_a_total() {
    let (mut q, mut r) = (Queue::new(), Scripted::default());
    let gone = gone(&mut q);
    let id = q.enqueue(request("a"));
    assert_eq!(q.fraction(id), Some(0.0));
    q.pump(&mut r);
    assert_eq!(q.fraction(id), None);
    q.apply(id, progress(10, None));
    assert_eq!(q.fraction(id), None);
    q.apply(id, progress(25, Some(100)));
    assert_eq!(q.fraction(id), Some(0.25));
    q.apply(id, progress(150, Some(100)));
    assert_eq!(q.fraction(id), Some(1.0));
    q.apply(id, progress(0, Some(0)));
    assert_eq!(q.fraction(id), Some(1.0));
    q.apply(id, RunnerEvent::Finished { frames: 150 });
    assert_eq!(q.fraction(id), Some(1.0));
    assert_eq!(q.fraction(gone), None);
}

#[test]
fn command_line_shows_what_the_cli_would_run() {
    let mut q = Queue::new();
    let gone = gone(&mut q);
    let id = q.enqueue(request("my song"));
    assert_eq!(
        q.command_line(id).unwrap(),
        "crossfade convert 'my song.wav' -o 'my song-out.wav'"
    );
    assert_eq!(q.command_line(gone), None);
    let mut odd = request("x");
    odd.job.inputs.clear();
    let id = q.enqueue(odd);
    assert_eq!(q.command_line(id), None);
}

#[test]
fn summary_counts_each_state() {
    let (mut q, mut r) = (Queue::new(), Scripted::default());
    let ids: Vec<_> = ["a", "b", "c", "d", "e"]
        .iter()
        .map(|n| q.enqueue(request(n)))
        .collect();
    q.pump(&mut r);
    q.apply(ids[0], RunnerEvent::Finished { frames: 1 });
    q.pump(&mut r);
    q.apply(
        ids[1],
        RunnerEvent::Failed {
            message: "x".into(),
        },
    );
    q.pump(&mut r);
    q.cancel(ids[3]);
    assert_eq!(
        q.summary(),
        Summary {
            pending: 1,
            running: 1,
            finished: 1,
            failed: 1,
            cancelled: 1
        }
    );
}

#[derive(Debug, Clone)]
enum Op {
    Enqueue,
    Pump,
    Cancel(usize),
    Retry(usize),
    Remove(usize),
    ClearDone,
    Event(usize, u8),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        Just(Op::Enqueue),
        Just(Op::Pump),
        (0..8usize).prop_map(Op::Cancel),
        (0..8usize).prop_map(Op::Retry),
        (0..8usize).prop_map(Op::Remove),
        Just(Op::ClearDone),
        (0..8usize, 0..6u8).prop_map(|(i, k)| Op::Event(i, k)),
    ]
}

fn is_terminal(s: &JobState) -> bool {
    matches!(
        s,
        JobState::Finished { .. } | JobState::Failed { .. } | JobState::Cancelled
    )
}

proptest! {
    #[test]
    fn random_interleavings_keep_one_runner_and_final_states_final(
        ops in prop::collection::vec(op(), 0..60)
    ) {
        let (mut q, mut r) = (Queue::new(), Scripted::default());
        let mut frozen: Vec<(JobId, JobState)> = Vec::new();
        for op in ops {
            let pick = |q: &Queue, i: usize| {
                let items = q.items();
                (!items.is_empty()).then(|| items[i % items.len()].id)
            };
            match op {
                Op::Enqueue => { q.enqueue(request("p")); }
                Op::Pump => q.pump(&mut r),
                Op::Cancel(i) => if let Some(id) = pick(&q, i) { q.cancel(id) },
                Op::Retry(i) => if let Some(id) = pick(&q, i) { q.retry(id); },
                Op::Remove(i) => if let Some(id) = pick(&q, i) { q.remove(id); },
                Op::ClearDone => q.clear_done(),
                Op::Event(i, kind) => if let Some(id) = pick(&q, i) {
                    q.apply(id, match kind {
                        0 => RunnerEvent::Started,
                        1 => progress(3, Some(10)),
                        2 => RunnerEvent::Finished { frames: 10 },
                        3 => RunnerEvent::Failed { message: "x".into() },
                        4 => RunnerEvent::Cancelled,
                        _ => progress(1, None),
                    });
                },
            }
            let running = q.items().iter().filter(|i| matches!(i.state, JobState::Running { .. })).count();
            prop_assert!(running <= 1);
            for (id, was) in &frozen {
                if let Some(item) = q.get(*id) {
                    prop_assert_eq!(&item.state, was);
                }
            }
            for item in q.items() {
                if is_terminal(&item.state) && !frozen.iter().any(|(id, _)| *id == item.id) {
                    frozen.push((item.id, item.state.clone()));
                }
            }
        }
    }
}
