//! Clean shutdown, retained schedulers and resource-release boundaries.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::future::Future;
use std::panic::{self, AssertUnwindSafe};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::task::{Context, Wake};

use futures::FutureExt;
use futures::executor::block_on;
use many_cpus::Processor;
use new_zealand::nz;
use testing::{assert_panics, with_watchdog};

use super::helpers::{
    OnDrop, PanickingPayload, TestPolicy, assert_retained_scheduler, hardware, pin, probe,
    regional_hardware,
};
use crate::worker_policy::{
    BindContext, BindError, BoundPolicy, WorkerGroupId, WorkerGroups, WorkerPolicy,
};
use crate::{BuiltInWorkerPolicy, GroupId, JoinHandle, Pool, Scheduler};

/// Records whether terminal admission ever reaches custom policy selection.
#[derive(Debug)]
struct RecordingPolicy {
    // This observes whether selection ran; runtime shutdown ordering is established by the
    // completed submission result rather than this counter.
    selected: Arc<AtomicUsize>,
}

impl WorkerPolicy for RecordingPolicy {
    fn bind(self, _context: BindContext<'_>) -> Result<impl BoundPolicy, BindError> {
        Ok(self)
    }
}

impl BoundPolicy for RecordingPolicy {
    fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
        self.selected.fetch_add(1, Ordering::Release);
        panic!("shutdown must prevent policy selection")
    }
}

/// A ready group without workers keeps both priority queues deterministic until shutdown.
#[test]
fn shutdown_preserves_joined_bodies_after_schedulers_are_dropped() {
    let policy = TestPolicy::new(0);
    let pool = Pool::builder()
        .hardware(hardware())
        .worker_policy(policy)
        .build()
        .unwrap();

    let scheduler = pool.scheduler();
    let dropped = Arc::new(AtomicUsize::new(0));
    let task = || {
        let capture = OnDrop(Some(Box::new({
            let dropped = Arc::clone(&dropped);
            move || {
                dropped.fetch_add(1, Ordering::Relaxed);
            }
        })));

        // Neither captured state nor the output needs Sync, Clone or Unpin.
        let value = Cell::new(String::from("recoverable"));
        let future = async {};
        move || {
            drop((capture, future));
            value
        }
    };

    let ordinary = scheduler.spawn(task());
    let urgent = scheduler.spawn_urgent(task());
    drop(scheduler);
    drop(pool);
    assert_eq!(dropped.load(Ordering::Relaxed), 0);

    for handle in [ordinary, urgent] {
        let error = handle.now_or_never().unwrap().err().unwrap();
        assert!(error.is_cancel());
        assert!(!error.is_panic());
        assert_eq!(error.into_body().unwrap()().into_inner(), "recoverable");
    }

    assert_eq!(dropped.load(Ordering::Relaxed), 2);
}

/// Completion callbacks run outside pool locks for execution and cancellation alike.
#[test]
fn completion_waker_can_drop_handles_and_submit() {
    /// Re-enters admission while disposing of the completed result and another pending handle.
    struct OnWake {
        handles: Mutex<Vec<JoinHandle<()>>>,
        scheduler: Scheduler,
        woke: AtomicUsize,
    }

    impl Wake for OnWake {
        fn wake(self: Arc<Self>) {
            self.handles.lock().unwrap().clear();
            self.scheduler.spawn_and_forget(|| {});
            self.woke.fetch_add(1, Ordering::Relaxed);
        }
    }

    for cancel in [false, true] {
        let policy = TestPolicy::new(0);
        let pool = Pool::builder()
            .hardware(hardware())
            .worker_policy(policy.clone())
            .build()
            .unwrap();
        let group = policy.group();
        let state = group.state();

        let scheduler = pool.scheduler();
        let mut handle = scheduler.spawn(|| {});
        let sibling = scheduler.spawn(|| {});
        let callback = Arc::new(OnWake {
            handles: Mutex::new(Vec::new()),
            scheduler,
            woke: AtomicUsize::new(0),
        });

        let waker = Arc::clone(&callback).into();
        assert!(
            Pin::new(&mut handle)
                .poll(&mut Context::from_waker(&waker))
                .is_pending()
        );
        callback.handles.lock().unwrap().extend([handle, sibling]);

        if cancel {
            drop(pool);
        } else {
            let mut task = state.regular_queue.lock().unwrap().pop_front().unwrap();
            task.as_pin_mut().call();
            drop(task);
            drop(pool);
        }

        assert_eq!(callback.woke.load(Ordering::Relaxed), 1);
        assert!(callback.handles.lock().unwrap().is_empty());
    }
}

/// Late registry initialization cannot escape the shutdown transition's startup gate.
#[test]
fn shutdown_before_lazy_state_creation_starts_no_workers() {
    let selected = Arc::new(AtomicUsize::new(0));
    let pool = Pool::builder()
        .hardware(hardware())
        .worker_policy(RecordingPolicy {
            selected: Arc::clone(&selected),
        })
        .build()
        .unwrap();
    let scheduler = pool.scheduler();
    let inner = Arc::clone(&pool.inner);
    drop(pool);

    let error = scheduler.spawn(|| {}).now_or_never().unwrap().unwrap_err();
    assert!(error.is_cancel());
    assert_eq!(selected.load(Ordering::Acquire), 0);
    assert_eq!(inner.runtime.active_workers(), 0);
}

/// Holds an allocated task across shutdown; its destructor probes every relevant lock.
#[test]
fn shutdown_racing_enqueue_drops_outside_locks() {
    let policy = TestPolicy::new(0);
    let pool = Pool::builder()
        .hardware(hardware())
        .worker_policy(policy.clone())
        .build()
        .unwrap();
    let inner = Arc::clone(&pool.inner);
    let group = policy.group();
    let state = group.state();
    let dropped = Arc::new(AtomicUsize::new(0));
    let task = probe(state, {
        let inner = Arc::clone(&inner);
        let dropped = Arc::clone(&dropped);
        let group = group.clone();
        move || {
            let state = group.state();
            drop(state.regular_queue.try_lock().unwrap());
            drop(state.task_pool.try_lock().unwrap());
            drop(inner.worker_handles.try_lock().unwrap());
            dropped.fetch_add(1, Ordering::Relaxed);
        }
    });

    drop(pool);
    inner.enqueue(state, task, false);

    assert_eq!(dropped.load(Ordering::Relaxed), 1);
    assert!(state.regular_queue.lock().unwrap().is_empty());
}

/// An urgent capture's destructor failure must not prevent normal-queue resource release.
#[test]
fn shutdown_drains_both_queues_despite_a_destructor_panic() {
    let policy = TestPolicy::new(0);
    let pool = Pool::builder()
        .hardware(hardware())
        .worker_policy(policy.clone())
        .build()
        .unwrap();
    let group = policy.group();
    let state = group.state();
    state
        .urgent_queue
        .lock()
        .unwrap()
        .push_back(probe(state, || panic!()));
    let dropped = Arc::new(AtomicUsize::new(0));
    state.regular_queue.lock().unwrap().push_back(probe(state, {
        let dropped = Arc::clone(&dropped);
        move || {
            dropped.fetch_add(1, Ordering::Relaxed);
        }
    }));

    assert_panics(|| drop(pool));

    assert_eq!(dropped.load(Ordering::Relaxed), 1);
    assert!(state.urgent_queue.lock().unwrap().is_empty());
    assert!(state.regular_queue.lock().unwrap().is_empty());
}

/// A secondary payload destructor panic takes precedence over the retained failure.
#[test]
fn shutdown_prioritizes_secondary_payload_destruction() {
    /// Identifies the first queued cleanup failure that must outrank later diagnostics.
    struct FirstFailure;

    let policy = TestPolicy::new(0);
    let pool = Pool::builder()
        .hardware(hardware())
        .worker_policy(policy.clone())
        .build()
        .unwrap();
    let group = policy.group();
    let state = group.state();
    let inner = Arc::clone(&pool.inner);
    state.urgent_queue.lock().unwrap().extend([
        probe(state, || panic::panic_any(FirstFailure)),
        probe(state, || panic::panic_any(PanickingPayload)),
    ]);

    let dropped = Arc::new(AtomicUsize::new(0));
    state.regular_queue.lock().unwrap().push_back(probe(state, {
        let dropped = Arc::clone(&dropped);
        move || {
            dropped.fetch_add(1, Ordering::Relaxed);
        }
    }));

    let payload = panic::catch_unwind(AssertUnwindSafe(|| drop(pool))).unwrap_err();

    assert!(payload.is::<PanickingPayload>());
    std::mem::forget(payload);
    assert_eq!(dropped.load(Ordering::Relaxed), 1);
    assert!(state.task_pool.lock().unwrap().is_empty());
    assert!(inner.runtime.is_poisoned());
}

/// A queued destructor re-enters admission and releases the running task during clean shutdown.
#[test]
fn retained_scheduler_releases_forgotten_captures_and_cancels_handles() {
    with_watchdog(|| {
        let pool = Pool::builder()
            .hardware(hardware())
            .worker_policy(TestPolicy::new(1))
            .build()
            .unwrap();

        let scheduler = pool.scheduler();
        let (entered, arrivals) = mpsc::channel();
        let (release, wait) = mpsc::channel();

        let running = scheduler.spawn(move || {
            entered.send(()).unwrap();
            wait.recv().unwrap();
        });
        arrivals.recv().unwrap();

        let dropped = Arc::new(AtomicUsize::new(0));
        let capture = OnDrop(Some(Box::new({
            let scheduler = scheduler.clone();
            let dropped = Arc::clone(&dropped);
            move || {
                // Reentrant admission must not deadlock or retain another task.
                let reentrant = scheduler.spawn(|| panic!());
                assert!(reentrant.now_or_never().unwrap().unwrap_err().is_cancel());
                dropped.fetch_add(1, Ordering::Relaxed);
                release.send(()).unwrap();
            }
        })));
        scheduler.spawn_and_forget(move || drop(capture));
        let abandoned = scheduler.spawn(|| panic!("must be cancelled"));
        let urgent = scheduler.spawn_urgent(|| panic!("must be abandoned"));

        drop(pool);
        block_on(running).unwrap();

        assert!(abandoned.now_or_never().unwrap().unwrap_err().is_cancel());
        assert!(urgent.now_or_never().unwrap().unwrap_err().is_cancel());
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        assert_retained_scheduler(&scheduler);
    });
}

/// Retaining a scheduler must preserve failure precedence after the pool owner is destroyed.
#[test]
fn poisoned_pool_rejects_submissions_even_after_drop() {
    let pool = Pool::builder()
        .hardware(hardware())
        .worker_policy(TestPolicy::new(0))
        .build()
        .unwrap();
    let scheduler = pool.scheduler();

    assert_panics(|| {
        pool.inner
            .submit((), |_| panic!("injected infrastructure failure"));
    });

    drop(pool);
    assert_panics(|| scheduler.spawn(|| {}));
    assert_panics(|| scheduler.spawn_urgent(|| {}));
    assert_panics(|| scheduler.spawn_and_forget(|| {}));
    assert_panics(|| scheduler.spawn_urgent_and_forget(|| {}));
}

/// A rejected group registration must not poison the terminal registry mutex.
#[test]
fn post_failure_group_registration_does_not_poison_cleanup() {
    let pool = Pool::builder().hardware(hardware()).build().unwrap();
    let inner = Arc::clone(&pool.inner);
    inner.stop_admission();

    assert_panics(|| {
        _ = inner.groups.add();
    });
    drop(pool);

    assert_eq!(inner.runtime.active_workers(), 0);
}

/// Clean shutdown cannot add system members, retain new work or restart the shared group.
#[test]
fn system_post_shutdown_submissions_do_not_expand_or_restart() {
    with_watchdog(|| {
        let hardware = hardware();
        pin(&hardware, 0);
        let pool = Pool::builder()
            .hardware(hardware.clone())
            .worker_policy(BuiltInWorkerPolicy::per_system(nz!(1)))
            .build()
            .unwrap();
        let scheduler = pool.scheduler();
        block_on(scheduler.spawn(|| {})).unwrap();

        let inner = Arc::clone(&pool.inner);
        drop(pool);

        pin(&hardware, 3);
        assert_retained_scheduler(&scheduler);

        let state = inner.registry.get_or_init(GroupId::System);
        assert_eq!(state.affinity_revision.load(Ordering::Acquire), 1);
        assert!(state.regular_queue.lock().unwrap().is_empty());
        assert!(state.urgent_queue.lock().unwrap().is_empty());
        assert_eq!(inner.runtime.active_workers(), 0);
        assert_eq!(
            state
                .affinity
                .read()
                .unwrap()
                .as_ref()
                .unwrap()
                .iter()
                .map(Processor::id)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([0])
        );
    });
}

/// Rejected submitters cannot expand an existing mask or initialize another region.
#[test]
fn post_shutdown_submissions_do_not_expand_or_activate() {
    let hardware = regional_hardware();
    pin(&hardware, 0);
    let pool = Pool::builder()
        .hardware(hardware.clone())
        .worker_policy(BuiltInWorkerPolicy::per_memory_region(nz!(1)))
        .build()
        .unwrap();
    let scheduler = pool.scheduler();
    block_on(scheduler.spawn(|| {})).unwrap();

    let inner = Arc::clone(&pool.inner);
    drop(pool);

    for processor in [1, 8] {
        pin(&hardware, processor);
        let task = scheduler.spawn(|| panic!("abandoned work must not execute"));
        assert!(task.now_or_never().unwrap().unwrap_err().is_cancel());
    }

    let existing = inner.registry.get_or_init(GroupId::MemoryRegion(2));
    assert_eq!(existing.affinity_revision.load(Ordering::Acquire), 1);
    let unused = inner.registry.get_or_init(GroupId::MemoryRegion(5));
    assert!(unused.startup.get().is_none());
    assert_eq!(unused.affinity_revision.load(Ordering::Acquire), 0);
    assert_eq!(inner.runtime.active_workers(), 0);
}
