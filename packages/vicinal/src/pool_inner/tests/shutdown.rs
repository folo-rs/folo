//! Clean shutdown, retained schedulers and resource-release boundaries.

use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;

use futures::executor::block_on;
use many_cpus::SystemHardware;
use many_cpus::fake::HardwareBuilder;
use new_zealand::nz;
use testing::{assert_panics, with_watchdog};

use super::helpers::{
    OnDrop, PanickingPayload, assert_retained_scheduler, hardware, pin, probe, regional_hardware,
};
use crate::{GroupId, Pool, SubmissionTarget, WorkerPolicy};

/// Late registry initialization cannot escape the shutdown transition's startup gate.
#[test]
fn shutdown_before_lazy_state_creation_starts_no_workers() {
    let pool = Pool::builder()
        .hardware(hardware())
        .worker_policy(WorkerPolicy::fixed_per_memory_region(nz!(1)))
        .build();
    let inner = Arc::clone(&pool.inner);
    drop(pool);
    let target = SubmissionTarget {
        group: GroupId::MemoryRegion(2),
        processor: 0,
    };
    let state = inner.registry.get_or_init(target.group);
    inner.enqueue_with(target, state, probe(state, || {}), false, |_| panic!());
    assert!(state.startup.get().is_none());
    assert!(inner.worker_handles.lock().unwrap().is_empty());
}

/// Holds an allocated task across shutdown; its destructor probes every relevant lock.
#[test]
fn shutdown_racing_enqueue_drops_outside_locks() {
    let pool = Pool::builder().hardware(hardware()).build();
    let inner = Arc::clone(&pool.inner);
    let destination = GroupId::Processor(0);
    let state = inner.registry.get_or_init(destination);
    let dropped = Arc::new(AtomicUsize::new(0));
    let task = probe(state, {
        let inner = Arc::clone(&inner);
        let dropped = Arc::clone(&dropped);
        move || {
            let state = inner.registry.get_or_init(destination);
            drop(state.regular_queue.try_lock().unwrap());
            drop(state.task_pool.try_lock().unwrap());
            drop(inner.worker_handles.try_lock().unwrap());
            dropped.fetch_add(1, Ordering::Relaxed);
        }
    });
    drop(pool);
    inner.enqueue(
        SubmissionTarget {
            group: destination,
            processor: 0,
        },
        state,
        task,
        false,
    );
    assert_eq!(dropped.load(Ordering::Relaxed), 1);
    assert!(state.regular_queue.lock().unwrap().is_empty());
}

/// An urgent capture's destructor failure must not prevent normal-queue resource release.
#[test]
fn shutdown_drains_both_queues_despite_a_destructor_panic() {
    let pool = Pool::builder().hardware(hardware()).build();
    let inner = Arc::clone(&pool.inner);
    let state = inner.registry.get_or_init(GroupId::Processor(0));
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

/// A secondary payload's destructor cannot stop queue draining or replace the first error.
#[test]
fn shutdown_contains_secondary_payload_destruction() {
    struct FirstFailure;

    let pool = Pool::builder().hardware(hardware()).build();
    let inner = Arc::clone(&pool.inner);
    let state = inner.registry.get_or_init(GroupId::Processor(0));
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
    assert!(payload.is::<FirstFailure>());
    assert_eq!(dropped.load(Ordering::Relaxed), 1);
    assert!(state.task_pool.lock().unwrap().is_empty());
    assert!(inner.poisoned.load(Ordering::Acquire));
}

/// Suppressing cleanup failure during an outer unwind must also contain its payload destructor.
#[test]
fn shutdown_contains_payload_destruction_during_outer_unwind() {
    struct OuterFailure;

    with_watchdog(|| {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
        let pool = Pool::builder().hardware(hardware).build();
        let inner = Arc::clone(&pool.inner);
        // Supply the cleanup failure through a joined worker rather than another nested task
        // unwind. This covers the same suppressed-payload boundary with less Miri backtrace work.
        inner
            .worker_handles
            .lock()
            .unwrap()
            .push(thread::spawn(|| panic::panic_any(PanickingPayload)));
        let payload = panic::catch_unwind(AssertUnwindSafe(move || {
            let _pool = pool;
            panic::panic_any(OuterFailure);
        }))
        .unwrap_err();
        assert!(payload.is::<OuterFailure>());
        assert!(inner.worker_handles.lock().unwrap().is_empty());
        assert!(inner.poisoned.load(Ordering::Acquire));
    });
}

/// A queued destructor re-enters admission and releases the running task during clean shutdown.
#[test]
fn retained_scheduler_releases_queued_captures_and_disconnects_handles() {
    with_watchdog(|| {
        let pool = Pool::builder()
            .hardware(hardware())
            .worker_policy(WorkerPolicy::fixed_per_memory_region(nz!(1)))
            .build();
        // Keep the submitter on one region throughout the shutdown scenario.
        let processors = pool.inner.hardware.all_processors();
        processors
            .to_builder()
            .filter(|p| p.id() == 0)
            .take_all()
            .unwrap()
            .pin_current_thread_to();
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
                assert_panics(|| block_on(reentrant));
                dropped.fetch_add(1, Ordering::Relaxed);
                release.send(()).unwrap();
            }
        })));
        let abandoned = scheduler.spawn(move || drop(capture));
        let urgent = scheduler.spawn_urgent(|| panic!("must be abandoned"));
        drop(pool);
        block_on(running);
        assert_panics(|| block_on(abandoned));
        assert_panics(|| block_on(urgent));
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        assert_retained_scheduler(&scheduler);
    });
}

/// Retaining a scheduler must preserve failure precedence after the pool owner is destroyed.
#[test]
fn poisoned_pool_rejects_submissions_even_after_drop() {
    for policy in [
        WorkerPolicy::fixed_per_processor(nz!(1)),
        WorkerPolicy::fixed_per_memory_region(nz!(1)),
    ] {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
        let destination = policy.destination(&hardware);
        let pool = Pool::builder()
            .hardware(hardware)
            .worker_policy(policy)
            .build();
        let scheduler = pool.scheduler();
        assert_panics(|| {
            let state = pool.inner.registry.get_or_init(destination.group);
            pool.inner
                .enqueue_with(destination, state, probe(state, || {}), false, |_| {
                    panic!("injected startup failure")
                });
        });
        drop(pool);
        assert_panics(|| scheduler.spawn(|| {}));
        assert_panics(|| scheduler.spawn_urgent(|| {}));
        assert_panics(|| scheduler.spawn_and_forget(|| {}));
        assert_panics(|| scheduler.spawn_urgent_and_forget(|| {}));
    }
}

/// Rejected submitters cannot expand an existing mask or initialize another region.
#[test]
fn post_shutdown_submissions_do_not_expand_or_activate() {
    let hardware = regional_hardware();
    pin(&hardware, 0);
    let pool = Pool::builder()
        .hardware(hardware.clone())
        .worker_policy(WorkerPolicy::fixed_per_memory_region(nz!(1)))
        .build();
    let scheduler = pool.scheduler();
    block_on(scheduler.spawn(|| {}));
    let inner = Arc::clone(&pool.inner);
    drop(pool);
    for processor in [1, 8] {
        pin(&hardware, processor);
        let task = scheduler.spawn(|| panic!("abandoned work must not execute"));
        assert_panics(|| block_on(task));
    }
    let existing = inner.registry.get_or_init(GroupId::MemoryRegion(2));
    assert_eq!(existing.affinity_revision.load(Ordering::Acquire), 1);
    let unused = inner.registry.get_or_init(GroupId::MemoryRegion(5));
    assert!(unused.startup.get().is_none());
    assert_eq!(unused.affinity_revision.load(Ordering::Acquire), 0);
    assert!(inner.worker_handles.lock().unwrap().is_empty());
}
