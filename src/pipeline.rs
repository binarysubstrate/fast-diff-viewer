use std::ops::Range;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use eframe::egui;

use crate::buffer_stamp::BufferStamp;
use crate::diff::{Role, assign_roles_and_emphasis, tokenize};

pub struct Job {
    pub revision: u64,
    pub left: String,
    pub right: String,
}

pub struct DiffResult {
    pub revision: u64,
    pub left_stamp: BufferStamp,
    pub right_stamp: BufferStamp,
    pub left_roles: Vec<Role>,
    pub right_roles: Vec<Role>,
    pub left_inline_emphasis: Vec<Vec<Range<usize>>>,
    pub right_inline_emphasis: Vec<Vec<Range<usize>>>,
}

/// Roles and inline emphasis ready to be applied to the panes, already
/// tagged with their revision so the renderer can enforce the revision-
/// aligned styled-render invariant.
pub struct AppliedRoles {
    // `revision` is kept for `LastPaintedStyledRevision`; some test
    // configurations intentionally do not read it.
    #[allow(dead_code)]
    pub revision: u64,
    pub left_stamp: BufferStamp,
    pub right_stamp: BufferStamp,
    pub left_roles: Vec<Role>,
    pub right_roles: Vec<Role>,
    pub left_inline_emphasis: Vec<Vec<Range<usize>>>,
    pub right_inline_emphasis: Vec<Vec<Range<usize>>>,
}

/// Async diff pipeline with bounded coalescing.
///
/// Invariants:
/// - At most one job is in flight at any time (`in_flight_revision`).
/// - At most one newer edit is queued behind it (`queued_revision`). A new
///   edit while in flight overwrites the queued slot; it does NOT send a new
///   job to the worker until the current in-flight result lands.
/// - Results older than `last_applied_revision` are dropped.
/// - When the in-flight result lands, the queued slot (if any) is promoted
///   and dispatched to the worker.
/// - Channel bound respected: at most one in-flight plus one queued.
pub struct DiffPipeline {
    pub doc_revision: u64,
    pub in_flight_revision: Option<u64>,
    pub queued_revision: Option<u64>,
    queued_left: Option<String>,
    queued_right: Option<String>,
    pub last_applied_revision: u64,
    tx: Sender<Job>,
    rx: Receiver<DiffResult>,
    // Liveness tracking. `worker` is `Some` in production and `None` in the
    // unit-test harness where no real thread is spawned.
    // `worker_alive` is sticky: once flipped to `false`, it stays there
    // for the rest of the pipeline's life — the architecture does not
    // support worker restart.
    worker: Option<thread::JoinHandle<()>>,
    worker_alive: bool,
}

impl DiffPipeline {
    pub fn spawn(ctx: egui::Context) -> Self {
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let (result_tx, result_rx) = mpsc::channel::<DiffResult>();
        let handle = thread::spawn(move || worker_loop(job_rx, result_tx, ctx));
        Self::from_parts(job_tx, result_rx, Some(handle))
    }

    fn from_parts(
        tx: Sender<Job>,
        rx: Receiver<DiffResult>,
        worker: Option<thread::JoinHandle<()>>,
    ) -> Self {
        Self {
            doc_revision: 0,
            in_flight_revision: None,
            queued_revision: None,
            queued_left: None,
            queued_right: None,
            last_applied_revision: 0,
            tx,
            rx,
            worker,
            worker_alive: true,
        }
    }

    #[cfg(test)]
    pub(crate) fn from_channels(tx: Sender<Job>, rx: Receiver<DiffResult>) -> Self {
        Self::from_parts(tx, rx, None)
    }

    /// Whether the background diff worker is still servicing jobs.
    ///
    /// `true` in steady state. Flips to `false` if the `JoinHandle`
    /// reports finished (worker panicked or returned) or a `Sender::send`
    /// fails because the worker's `Receiver<Job>` was dropped. Sticky —
    /// never flips back. UI chrome should surface this to the user.
    pub fn is_worker_alive(&self) -> bool {
        self.worker_alive
    }

    /// Opportunistic liveness probe via the `JoinHandle`. No-op in the
    /// test harness, where `worker` is `None`.
    fn check_worker_handle(&mut self) {
        if self.worker_alive
            && let Some(handle) = &self.worker
            && handle.is_finished()
        {
            self.worker_alive = false;
        }
    }

    /// Send a job to the worker, promoting any `Sender::send` error to
    /// the sticky `worker_alive = false` state. When the worker is
    /// already known dead, skip the send entirely — do not silently
    /// swallow a `SendError` as if the send had succeeded. Returns
    /// `true` only if the job was actually handed off.
    fn try_send_job(&mut self, job: Job) -> bool {
        self.check_worker_handle();
        if !self.worker_alive {
            return false;
        }
        if self.tx.send(job).is_err() {
            self.worker_alive = false;
            return false;
        }
        true
    }

    /// Record a new edit. Increments `doc_revision`, and either sends a job
    /// immediately (if the worker is idle) or overwrites the queued slot.
    /// Returns the new `doc_revision`.
    ///
    /// If the worker is known dead, `doc_revision` still advances (callers
    /// use it as a monotonic invariant) but no in-flight / queued state is
    /// updated — pretending to enqueue a job against a dead worker is
    /// a silent-failure mode where the UI would keep accepting edits while
    /// no diff work could ever run.
    pub fn dispatch(&mut self, left: &str, right: &str) -> u64 {
        self.doc_revision += 1;
        let rev = self.doc_revision;
        self.check_worker_handle();
        if !self.worker_alive {
            return rev;
        }
        if self.in_flight_revision.is_none() {
            self.in_flight_revision = Some(rev);
            if !self.try_send_job(Job {
                revision: rev,
                left: left.to_string(),
                right: right.to_string(),
            }) {
                // Send failed at this moment — worker died between the
                // handle probe and the send. Clear the optimistic
                // in_flight so no phantom revision sits in the pipeline.
                self.in_flight_revision = None;
            }
        } else {
            self.queued_revision = Some(rev);
            self.queued_left = Some(left.to_string());
            self.queued_right = Some(right.to_string());
        }
        rev
    }

    /// Drain every result currently available. Drops anything older than
    /// `last_applied_revision`. Returns the newest applied result, if any.
    /// If the in-flight revision just landed and the queued slot is set,
    /// promotes it and dispatches a new job.
    pub fn drain_results(&mut self) -> Option<AppliedRoles> {
        self.check_worker_handle();
        let mut applied: Option<AppliedRoles> = None;
        while let Ok(result) = self.rx.try_recv() {
            if result.revision <= self.last_applied_revision {
                continue;
            }
            self.last_applied_revision = result.revision;
            if self.in_flight_revision == Some(result.revision) {
                if let Some(qrev) = self.queued_revision.take() {
                    let qleft = self.queued_left.take().unwrap_or_default();
                    let qright = self.queued_right.take().unwrap_or_default();
                    self.in_flight_revision = Some(qrev);
                    if !self.try_send_job(Job {
                        revision: qrev,
                        left: qleft,
                        right: qright,
                    }) {
                        self.in_flight_revision = None;
                    }
                } else {
                    self.in_flight_revision = None;
                }
            }
            applied = Some(AppliedRoles {
                revision: result.revision,
                left_stamp: result.left_stamp,
                right_stamp: result.right_stamp,
                left_roles: result.left_roles,
                right_roles: result.right_roles,
                left_inline_emphasis: result.left_inline_emphasis,
                right_inline_emphasis: result.right_inline_emphasis,
            });
        }
        applied
    }
}

pub fn worker_loop(jobs: Receiver<Job>, results: Sender<DiffResult>, ctx: egui::Context) {
    while let Ok(job) = jobs.recv() {
        let left_stamp = BufferStamp::from_text(job.revision, &job.left);
        let right_stamp = BufferStamp::from_text(job.revision, &job.right);
        let left_tokens = tokenize(&job.left);
        let right_tokens = tokenize(&job.right);
        let (left_roles, right_roles, left_inline_emphasis, right_inline_emphasis) =
            assign_roles_and_emphasis(&left_tokens, &right_tokens);
        let result = DiffResult {
            revision: job.revision,
            left_stamp,
            right_stamp,
            left_roles,
            right_roles,
            left_inline_emphasis,
            right_inline_emphasis,
        };
        if results.send(result).is_err() {
            break;
        }
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_pipeline() -> (DiffPipeline, Sender<DiffResult>, Receiver<Job>) {
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let (result_tx, result_rx) = mpsc::channel::<DiffResult>();
        let pipeline = DiffPipeline::from_channels(job_tx, result_rx);
        (pipeline, result_tx, job_rx)
    }

    fn send_result(tx: &Sender<DiffResult>, rev: u64) {
        tx.send(DiffResult {
            revision: rev,
            left_stamp: BufferStamp::from_text(rev, ""),
            right_stamp: BufferStamp::from_text(rev, ""),
            left_roles: vec![],
            right_roles: vec![],
            left_inline_emphasis: vec![],
            right_inline_emphasis: vec![],
        })
        .unwrap();
    }

    #[test]
    fn initial_state_is_clean() {
        let (pipeline, _, _) = test_pipeline();
        assert_eq!(pipeline.doc_revision, 0);
        assert_eq!(pipeline.in_flight_revision, None);
        assert_eq!(pipeline.queued_revision, None);
        assert_eq!(pipeline.last_applied_revision, 0);
    }

    #[test]
    fn first_dispatch_sends_job_and_marks_in_flight() {
        let (mut pipeline, _result_tx, job_rx) = test_pipeline();
        let rev = pipeline.dispatch("a", "b");
        assert_eq!(rev, 1);
        assert_eq!(pipeline.in_flight_revision, Some(1));
        assert_eq!(pipeline.queued_revision, None);

        let job = job_rx.try_recv().expect("first dispatch sends a job");
        assert_eq!(job.revision, 1);
        assert_eq!(job.left, "a");
        assert_eq!(job.right, "b");
        assert!(job_rx.try_recv().is_err());
    }

    #[test]
    fn dispatch_while_in_flight_queues_and_does_not_send_job() {
        let (mut pipeline, _result_tx, job_rx) = test_pipeline();
        pipeline.dispatch("a", "");
        // Drain the first job so the inbox is clear.
        let _ = job_rx.try_recv().unwrap();

        pipeline.dispatch("ab", "");
        assert_eq!(pipeline.doc_revision, 2);
        assert_eq!(pipeline.in_flight_revision, Some(1));
        assert_eq!(pipeline.queued_revision, Some(2));
        assert!(
            job_rx.try_recv().is_err(),
            "no new job should be sent while one is in flight"
        );
    }

    #[test]
    fn bounded_queue_overwrites_on_new_edits_while_in_flight() {
        let (mut pipeline, _result_tx, job_rx) = test_pipeline();
        for _ in 0..50 {
            pipeline.dispatch("x", "y");
        }
        assert_eq!(pipeline.doc_revision, 50);
        assert_eq!(pipeline.in_flight_revision, Some(1));
        assert_eq!(
            pipeline.queued_revision,
            Some(50),
            "queued slot must hold the newest edit, overwriting older ones"
        );

        // Exactly one job was sent: the first one.
        let first = job_rx.try_recv().unwrap();
        assert_eq!(first.revision, 1);
        assert!(
            job_rx.try_recv().is_err(),
            "channel must not accumulate intermediate jobs"
        );
    }

    #[test]
    fn result_for_in_flight_promotes_queued_and_dispatches_latest_buffers() {
        let (mut pipeline, result_tx, job_rx) = test_pipeline();

        pipeline.dispatch("rev1-left", "rev1-right");
        let first = job_rx.try_recv().unwrap();
        assert_eq!(first.revision, 1);

        // Two more edits while in flight; queued overwrites to rev 3.
        pipeline.dispatch("rev2-left", "rev2-right");
        pipeline.dispatch("rev3-left", "rev3-right");
        assert_eq!(pipeline.queued_revision, Some(3));
        assert!(job_rx.try_recv().is_err());

        // Worker finishes rev 1.
        send_result(&result_tx, 1);
        let applied = pipeline.drain_results().unwrap();
        assert_eq!(applied.revision, 1);

        // Queued promoted to in-flight; a new job was sent with rev 3's buffers.
        assert_eq!(pipeline.in_flight_revision, Some(3));
        assert_eq!(pipeline.queued_revision, None);
        let promoted = job_rx.try_recv().unwrap();
        assert_eq!(promoted.revision, 3);
        assert_eq!(promoted.left, "rev3-left");
        assert_eq!(promoted.right, "rev3-right");
        assert!(job_rx.try_recv().is_err());
    }

    #[test]
    fn result_for_only_in_flight_clears_state_when_queue_empty() {
        let (mut pipeline, result_tx, job_rx) = test_pipeline();
        pipeline.dispatch("a", "");
        let _ = job_rx.try_recv().unwrap();

        send_result(&result_tx, 1);
        let applied = pipeline.drain_results().unwrap();
        assert_eq!(applied.revision, 1);
        assert_eq!(pipeline.in_flight_revision, None);
        assert_eq!(pipeline.queued_revision, None);
        assert_eq!(pipeline.last_applied_revision, 1);
        assert!(
            job_rx.try_recv().is_err(),
            "no follow-up job when nothing queued"
        );
    }

    #[test]
    fn burst_edit_determinism_final_applied_revision_is_last_edit() {
        let (mut pipeline, result_tx, job_rx) = test_pipeline();

        for _ in 0..50 {
            pipeline.dispatch("l", "r");
        }
        assert_eq!(pipeline.doc_revision, 50);
        // Exactly one job sent — rev 1.
        let first = job_rx.try_recv().unwrap();
        assert_eq!(first.revision, 1);
        assert!(job_rx.try_recv().is_err());

        // Worker finishes rev 1 → promotes queued rev 50.
        send_result(&result_tx, 1);
        let applied = pipeline.drain_results().unwrap();
        assert_eq!(applied.revision, 1);
        assert_eq!(pipeline.in_flight_revision, Some(50));
        let second = job_rx.try_recv().unwrap();
        assert_eq!(second.revision, 50);
        assert!(job_rx.try_recv().is_err());

        // Worker finishes rev 50 → terminal clean state.
        send_result(&result_tx, 50);
        let applied = pipeline.drain_results().unwrap();
        assert_eq!(applied.revision, 50);
        assert_eq!(pipeline.last_applied_revision, 50);
        assert_eq!(pipeline.in_flight_revision, None);
        assert_eq!(pipeline.queued_revision, None);
        assert!(job_rx.try_recv().is_err());
    }

    #[test]
    fn drain_drops_stale_results() {
        let (mut pipeline, result_tx, _job_rx) = test_pipeline();
        pipeline.dispatch("a", "");
        send_result(&result_tx, 1);
        pipeline.drain_results();
        assert_eq!(pipeline.last_applied_revision, 1);

        // A stale result for an earlier revision must be dropped.
        send_result(&result_tx, 1);
        let applied = pipeline.drain_results();
        assert!(
            applied.is_none(),
            "revision <= last_applied must be dropped"
        );
        assert_eq!(pipeline.last_applied_revision, 1);
    }

    #[test]
    fn drain_with_no_results_is_noop() {
        let (mut pipeline, _result_tx, _job_rx) = test_pipeline();
        assert!(pipeline.drain_results().is_none());
        assert_eq!(pipeline.doc_revision, 0);
        assert_eq!(pipeline.last_applied_revision, 0);
    }

    #[test]
    fn initial_state_reports_worker_alive() {
        let (pipeline, _, _) = test_pipeline();
        assert!(
            pipeline.is_worker_alive(),
            "freshly constructed pipeline must report worker alive"
        );
    }

    #[test]
    fn dropping_job_receiver_flips_worker_alive_on_next_dispatch() {
        let (mut pipeline, _result_tx, job_rx) = test_pipeline();

        // Simulate the worker dying: its `Receiver<Job>` is gone, so any
        // future `Sender::send` will fail with `SendError`.
        drop(job_rx);

        assert!(
            pipeline.is_worker_alive(),
            "worker_alive must still be true until dispatch probes the channel"
        );
        let rev = pipeline.dispatch("a", "b");
        assert_eq!(rev, 1, "doc_revision must still advance past worker death");
        assert!(
            !pipeline.is_worker_alive(),
            "Sender::send failure during dispatch must flip worker_alive"
        );
        // Invariant: once dead, no phantom in-flight / queued state is
        // left behind. `dispatch` optimistically sets in_flight before
        // the send; when the send fails, the optimistic state is torn
        // down. A dead worker must never leave a phantom revision
        // sitting in the pipeline forever.
        assert_eq!(
            pipeline.in_flight_revision, None,
            "failed send must clear the optimistic in_flight slot"
        );
        assert_eq!(
            pipeline.queued_revision, None,
            "no queued work against a dead worker"
        );
    }

    #[test]
    fn dropping_job_receiver_flips_worker_alive_on_next_drain() {
        let (mut pipeline, result_tx, job_rx) = test_pipeline();

        // Dispatch one job (this send succeeds — the receiver is still
        // alive), then kill the worker and queue a second edit. When the
        // first result lands, the queued promotion will try to send and
        // fail, flipping the liveness flag.
        pipeline.dispatch("rev1", "rev1");
        let _ = job_rx.try_recv().unwrap();
        pipeline.dispatch("rev2", "rev2");
        assert_eq!(pipeline.queued_revision, Some(2));
        drop(job_rx);

        send_result(&result_tx, 1);
        let applied = pipeline.drain_results();
        assert!(
            applied.is_some(),
            "result for rev 1 must still be applied — worker death does not poison already-landed results"
        );
        assert!(
            !pipeline.is_worker_alive(),
            "queued-promotion send failure during drain must flip worker_alive"
        );
    }

    #[test]
    fn worker_alive_is_sticky_once_flipped() {
        let (mut pipeline, result_tx, job_rx) = test_pipeline();
        drop(job_rx);
        pipeline.dispatch("a", "b");
        assert!(!pipeline.is_worker_alive());

        // Further dispatches / drains on an empty queue must not flicker
        // the flag back to alive. The sticky invariant is what lets the
        // UI paint a stable "diff offline" indicator.
        for _ in 0..5 {
            pipeline.dispatch("x", "y");
            assert!(
                !pipeline.is_worker_alive(),
                "worker_alive must remain false across repeated dispatches"
            );
        }
        for _ in 0..5 {
            let _ = pipeline.drain_results();
            assert!(
                !pipeline.is_worker_alive(),
                "worker_alive must remain false across repeated drains"
            );
        }
        // Even an unrelated result landing cannot resuscitate the
        // liveness flag — the result channel is still alive in this
        // test but the worker is dead by definition.
        send_result(&result_tx, 99);
        let _ = pipeline.drain_results();
        assert!(
            !pipeline.is_worker_alive(),
            "landing a result must not resurrect worker_alive"
        );
    }

    #[test]
    fn dispatch_with_live_worker_remains_unchanged() {
        // Regression guard for the liveness check in dispatch /
        // drain_results. Every existing normal-path test
        // above this block exercises the same semantics, but make the
        // guarantee explicit: a single dispatch against a live pipeline
        // still sends the job and still returns rev = 1.
        let (mut pipeline, _result_tx, job_rx) = test_pipeline();
        let rev = pipeline.dispatch("a", "b");
        assert_eq!(rev, 1);
        assert_eq!(pipeline.in_flight_revision, Some(1));
        assert!(pipeline.is_worker_alive());
        let job = job_rx.try_recv().expect("live pipeline must still send");
        assert_eq!(job.revision, 1);
    }

    #[test]
    fn worker_loop_processes_one_job_and_returns_aligned_result() {
        // Exercise the full send/receive path of `worker_loop`. Spawn the
        // worker on its own thread
        // (the function is blocking), send a single job whose buffers
        // share `a` but diverge on `b` vs `c`, and assert the returned
        // `DiffResult` carries the expected revision and produces
        // segment-aligned role and emphasis vectors.
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let (result_tx, result_rx) = mpsc::channel::<DiffResult>();
        let ctx = egui::Context::default();
        let worker = thread::spawn(move || worker_loop(job_rx, result_tx, ctx));

        let left = "a\nb";
        let right = "a\nc";
        job_tx
            .send(Job {
                revision: 7,
                left: left.to_string(),
                right: right.to_string(),
            })
            .unwrap();
        let result = result_rx
            .recv()
            .expect("worker must emit one result per job");
        assert_eq!(result.revision, 7);
        assert_eq!(result.left_stamp, BufferStamp::from_text(7, left));
        assert_eq!(result.right_stamp, BufferStamp::from_text(7, right));
        // Two segments on each side; second is a paired Changed.
        assert_eq!(result.left_roles.len(), 2);
        assert_eq!(result.right_roles.len(), 2);
        assert_eq!(result.left_inline_emphasis.len(), 2);
        assert_eq!(result.right_inline_emphasis.len(), 2);
        assert_eq!(result.left_roles[0], crate::diff::Role::Equal);
        assert_eq!(result.right_roles[0], crate::diff::Role::Equal);
        assert_eq!(result.left_roles[1], crate::diff::Role::Changed);
        assert_eq!(result.right_roles[1], crate::diff::Role::Changed);
        // Dropping `job_tx` closes the channel so the worker returns
        // cleanly, preventing the test from leaking a thread.
        drop(job_tx);
        worker.join().expect("worker thread must exit cleanly");
    }

    #[test]
    fn drain_results_preserves_source_stamps_in_applied_roles() {
        let (mut pipeline, result_tx, job_rx) = test_pipeline();
        pipeline.dispatch("old left", "old right");
        let _ = job_rx.try_recv().unwrap();

        let left_stamp = BufferStamp::from_text(1, "old left");
        let right_stamp = BufferStamp::from_text(1, "old right");
        result_tx
            .send(DiffResult {
                revision: 1,
                left_stamp,
                right_stamp,
                left_roles: vec![Role::Changed],
                right_roles: vec![Role::Changed],
                left_inline_emphasis: vec![vec![0..3]],
                right_inline_emphasis: vec![vec![4..9]],
            })
            .unwrap();

        let applied = pipeline.drain_results().unwrap();
        assert_eq!(applied.left_stamp, left_stamp);
        assert_eq!(applied.right_stamp, right_stamp);
    }
}
