//! Pool-wide infrastructure failure and user-task panic isolation.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::{panic, thread};

use futures::executor::block_on;
use new_zealand::nz;
use testing::{assert_panics, with_watchdog};

use super::helpers::{CountOnDrop, NoopTask, PanickingPayload, hardware, pin, regional_hardware};
use crate::{Pool, WorkerPolicy, init_task};

/// Submission failures stop admission, whereas caught user-task panics leave it operational.
#[test]
fn submission_infrastructure_failure_poisons_pool_but_task_panics_do_not() {
    with_watchdog(|| {
        let pool = Pool::builder()
            .hardware(hardware())
            .worker_policy(WorkerPolicy::fixed_per_processor(nz!(1)))
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
