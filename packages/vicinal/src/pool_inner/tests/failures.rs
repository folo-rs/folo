//! Pool-wide infrastructure failure and user-task panic isolation.

use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::{panic, thread};

use futures::executor::block_on;
use new_zealand::nz;
use testing::{assert_panics, with_watchdog};

use super::helpers::{
    CountOnDrop, NoopTask, PanickingPayload, hardware, pin, probe, regional_hardware,
};
use crate::pool_inner::refresh_affinity;
use crate::{Pool, WorkerCore, WorkerPolicy, init_task};

/// Submission failures stop admission, whereas caught user-task panics leave it operational.
#[test]
fn submission_infrastructure_failure_poisons_pool_but_task_panics_do_not() {
    with_watchdog(|| {
        let pool = Pool::builder()
            .hardware(hardware())
            .workers_per_processor(nz!(1))
            .build();
        let scheduler = pool.scheduler();
        assert_panics(|| block_on(scheduler.spawn(|| panic!("task panic"))));
        scheduler.spawn_and_forget(|| panic!("fire-and-forget task panic"));
        scheduler.spawn_and_forget(|| panic::panic_any(PanickingPayload));
        block_on(scheduler.spawn(|| {}));
        assert!(!pool.inner.poisoned.load(Ordering::Acquire));

        // This is the common boundary for topology, result-channel and task-slot failures.
        assert_panics(|| {
            pool.inner
                .submit((), |_| panic!("injected infrastructure failure"))
        });
        assert!(pool.inner.poisoned.load(Ordering::Acquire));
        assert!(pool.inner.shutdown.load(Ordering::Acquire));
        assert_panics(|| scheduler.spawn(|| {}));
    });
}

/// Drop the receiver while its task is blocked so the worker must discard the eventual panic.
#[test]
fn disconnected_joined_panic_does_not_poison_pool() {
    with_watchdog(|| {
        let pool = Pool::builder()
            .hardware(hardware())
            .worker_policy(WorkerPolicy::fixed_per_processor(nz!(1)))
            .build();
        pin(&pool.inner.hardware, 0);
        let scheduler = pool.scheduler();
        let (release, wait) = mpsc::channel();
        let abandoned = scheduler.spawn(move || {
            wait.recv().unwrap();
            panic::panic_any(PanickingPayload);
        });
        drop(abandoned);
        release.send(()).unwrap();
        block_on(scheduler.spawn(|| {}));
        assert!(!pool.inner.poisoned.load(Ordering::Acquire));
    });
}

/// Finish delivery first to cover payload destruction on the receiver instead of the worker.
#[test]
fn unobserved_joined_panic_can_be_dropped_after_completion() {
    with_watchdog(|| {
        let pool = Pool::builder()
            .hardware(hardware())
            .worker_policy(WorkerPolicy::fixed_per_processor(nz!(1)))
            .build();
        pin(&pool.inner.hardware, 0);
        let scheduler = pool.scheduler();
        let completed = scheduler.spawn(|| panic::panic_any(PanickingPayload));
        block_on(scheduler.spawn(|| {}));
        drop(completed);
        assert!(!pool.inner.poisoned.load(Ordering::Acquire));
    });
}

/// Dequeued-task destruction must see pool-wide rejection, not admit work to a failing worker.
#[test]
fn dequeued_refresh_failure_rejects_reentrant_submissions_before_cleanup() {
    struct AffinityFailure;

    with_watchdog(|| {
        let hardware = hardware();
        pin(&hardware, 0);
        let pool = Pool::builder()
            .hardware(hardware.clone())
            .worker_policy(WorkerPolicy::fixed_per_memory_region(nz!(1)))
            .build();
        let target = pool.inner.policy.destination(&hardware);
        let state = pool.inner.registry.get_or_init(target.group);
        let affinity = hardware
            .all_processors()
            .to_builder()
            .filter(|processor| processor.id() == target.processor)
            .take_all()
            .unwrap();
        *state.affinity.lock().unwrap() = Some(Arc::new(affinity));
        // Publish a snapshot to exercise the refresh after dequeueing, independently of startup.
        state.affinity_revision.store(1, Ordering::Release);

        let observed_stop = Arc::new(AtomicBool::new(false));
        let rejected = Arc::new(AtomicBool::new(false));
        state.regular_queue.lock().unwrap().push_back(probe(state, {
            let scheduler = pool.scheduler();
            let inner = Arc::clone(&pool.inner);
            let observed_stop = Arc::clone(&observed_stop);
            let rejected = Arc::clone(&rejected);
            move || {
                observed_stop.store(
                    inner.poisoned.load(Ordering::Acquire)
                        && inner.shutdown.load(Ordering::Acquire),
                    Ordering::Release,
                );
                assert_panics(|| scheduler.spawn(|| {}));
                assert_panics(|| scheduler.spawn_urgent(|| {}));
                assert_panics(|| scheduler.spawn_and_forget(|| {}));
                assert_panics(|| scheduler.spawn_urgent_and_forget(|| {}));
                rejected.store(true, Ordering::Release);
            }
        }));
        let core = WorkerCore::new(
            &state.urgent_queue,
            &state.regular_queue,
            &state.shutdown_flag,
        );
        let mut revision = 0;
        let payload = panic::catch_unwind(AssertUnwindSafe(|| {
            core.run_one_iteration_with(
                || {
                    refresh_affinity(state, &mut revision, |_| panic::panic_any(AffinityFailure));
                    true
                },
                || pool.inner.stop_admission(),
            )
        }))
        .unwrap_err();
        assert!(payload.is::<AffinityFailure>());
        assert!(observed_stop.load(Ordering::Acquire));
        assert!(rejected.load(Ordering::Acquire));
        assert!(pool.inner.worker_handles.lock().unwrap().is_empty());
        assert!(state.task_pool.lock().unwrap().is_empty());
    });
}

/// A failed refresh drains every group while a blocked, already-running task may still finish.
#[test]
fn refresh_failure_stops_pool_and_disconnects_accepted_work() {
    with_watchdog(|| {
        let hardware = regional_hardware();
        pin(&hardware, 0);
        let pool = Pool::builder()
            .hardware(hardware.clone())
            .worker_policy(WorkerPolicy::fixed_per_memory_region(nz!(1)))
            .build();
        let target = pool.inner.policy.destination(&hardware);
        let state = pool.inner.registry.get_or_init(target.group);
        let slot = state.task_pool.lock().unwrap().alloc_uninit_box();
        pool.inner
            .enqueue_with(target, state, init_task(slot, NoopTask), false, |handles| {
                pool.inner
                    .start_workers(target, handles, thread::Builder::spawn, |set| {
                        assert_eq!(set.len(), 1);
                        set.pin_current_thread_to();
                    });
            });
        let scheduler = pool.scheduler();
        let (entered, arrivals) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let running = scheduler.spawn(move || {
            entered.send(()).unwrap();
            wait.recv().unwrap();
        });
        arrivals.recv().unwrap();
        let dropped = Arc::new(AtomicUsize::new(0));
        let task = || {
            let capture = CountOnDrop {
                dropped: Arc::clone(&dropped),
            };
            move || drop(capture)
        };
        let ordinary = scheduler.spawn(task());
        pin(&hardware, 8);
        let (other_entered, other_arrivals) = mpsc::channel();
        let (other_release, other_wait) = mpsc::channel();
        let other_running = scheduler.spawn(move || {
            other_entered.send(()).unwrap();
            other_wait.recv().unwrap();
        });
        other_arrivals.recv().unwrap();
        let other_queued = scheduler.spawn(task());
        pin(&hardware, 1);
        let expansion = scheduler.spawn_urgent(task());
        release.send(()).unwrap();
        block_on(running);
        // Failure closes and drains the queues before it becomes merely a Pool-drop concern.
        assert_panics(|| block_on(ordinary));
        assert_panics(|| block_on(expansion));
        assert_panics(|| block_on(other_queued));
        assert_eq!(dropped.load(Ordering::Acquire), 3);
        pin(&hardware, 8);
        assert_panics(|| scheduler.spawn_and_forget(|| {}));
        other_release.send(()).unwrap();
        block_on(other_running);
        assert_panics(|| drop(pool));
        assert_panics(|| scheduler.spawn(|| {}));
    });
}
