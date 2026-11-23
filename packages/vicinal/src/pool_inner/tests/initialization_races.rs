//! Controlled initializer failures and shutdown at worker-registration boundaries.

use std::panic::{AssertUnwindSafe, catch_unwind, panic_any};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::thread::spawn;

use futures::executor::block_on;
use many_cpus::ProcessorSet;
use new_zealand::nz;
use testing::{assert_panics, with_watchdog};

use super::helpers::{NoopTask, OnDrop, TestPolicy, hardware, pin};
use crate::{BuiltInWorkerPolicy, Pool, init_task};

/// Initializer failure cannot lose a sibling's handle while startup still owns registration.
#[test]
fn initialization_failure_during_sibling_startup_joins_every_worker() {
    // A sibling remains to be registered when the first initializer fails.
    const WORKERS: u32 = 2;

    with_watchdog(|| {
        for policy in [
            BuiltInWorkerPolicy::per_processor(nz!(WORKERS)),
            BuiltInWorkerPolicy::per_memory_region(nz!(WORKERS)),
        ] {
            let hardware = hardware();
            let target = policy.destination(&hardware);
            let initialized = Arc::new(AtomicUsize::new(0));
            let (failed, failure) = channel();
            let pool = Pool::builder()
                .hardware(hardware)
                .worker_policy(policy)
                .thread_init({
                    let initialized = Arc::clone(&initialized);
                    move || {
                        if initialized.fetch_add(1, Ordering::Relaxed) == 0 {
                            let _unwinding = OnDrop(Some(Box::new({
                                let failed = failed.clone();
                                move || failed.send(()).unwrap()
                            })));
                            panic!("injected initialization failure");
                        }
                    }
                })
                .build();

            let inner = Arc::clone(&pool.inner);
            let scheduler = pool.scheduler();
            let state = inner.registry.get_or_init(target.group);
            let slot = state.task_pool.lock().unwrap().alloc_uninit_box();
            let mut spawned = 0_u32;

            // Admission may observe the concurrent failure or finish before poisoning is published.
            let submission = catch_unwind(AssertUnwindSafe(|| {
                inner.enqueue_with(target, state, init_task(slot, NoopTask), false, |handles| {
                    inner.start_workers(
                        target,
                        nz!(WORKERS),
                        handles,
                        |builder, entry| {
                            spawned = spawned.checked_add(1).unwrap();
                            if spawned == WORKERS {
                                // The earlier callback must unwind before its sibling is created.
                                failure.recv().unwrap();
                            }
                            builder.spawn(entry)
                        },
                        ProcessorSet::pin_current_thread_to,
                    );
                });
            }));

            assert_eq!(spawned, WORKERS);
            inner.runtime.wait_for_workers();
            let worker_count = inner.runtime.active_workers();
            assert_eq!(worker_count, 0);

            if submission.is_err() {
                // The startup call consumed and propagated the original infrastructure
                // failure, leaving clean pool destruction to perform terminal cleanup.
                drop(pool);
            } else {
                // Startup reached readiness before failure publication, so the original
                // retained failure is propagated by pool destruction instead.
                assert_panics(|| drop(pool));
            }
            let initialized = initialized.load(Ordering::Relaxed);
            assert!((1..=WORKERS as usize).contains(&initialized));
            assert_eq!(inner.runtime.active_workers(), 0);
            assert!(state.task_pool.lock().unwrap().is_empty());
            assert!(inner.runtime.is_poisoned());
            assert_panics(|| scheduler.spawn(|| {}));
        }
    });
}

/// Pool destruction must not finish between a caught failure and participation release.
#[test]
fn shutdown_waits_for_failure_publication_to_finish() {
    struct InitializationFailure;

    with_watchdog(|| {
        let hardware = hardware();
        pin(&hardware, 0);
        let policy = TestPolicy::new(0);
        let (initializer_entered, initializer_arrival) = channel();
        let (release_initializer, wait_for_release) = channel();
        let pool = Pool::builder()
            .hardware(hardware)
            .worker_policy(policy.clone())
            .thread_init({
                let initializer_entered = initializer_entered;
                let wait_for_release = Mutex::new(wait_for_release);
                move || {
                    initializer_entered.send(()).unwrap();
                    wait_for_release.lock().unwrap().recv().unwrap();
                    panic_any(InitializationFailure);
                }
            })
            .build();
        let inner = Arc::clone(&pool.inner);
        let group = policy.group();
        let state = group.state();

        let (failure_entered, failure_arrival) = channel();
        let (release_failure, wait_for_failure_release) = channel();
        let wait_for_failure_release = Mutex::new(wait_for_failure_release);
        assert!(
            state
                .before_failure_exit
                .set(Box::new(move || {
                    failure_entered.send(()).unwrap();
                    wait_for_failure_release.lock().unwrap().recv().unwrap();
                }))
                .is_ok()
        );

        let worker = spawn({
            let group = group.clone();
            move || group.execute_worker()
        });
        initializer_arrival.recv().unwrap();

        let scheduler = pool.scheduler();
        let pending = scheduler.spawn(|| ());
        release_initializer.send(()).unwrap();
        failure_arrival.recv().unwrap();

        assert!(inner.runtime.is_poisoned());
        assert_eq!(inner.runtime.active_workers(), 1);

        let (shutdown_entered, shutdown_arrival) = channel();
        let (shutdown_done, shutdown_completion) = channel();
        let shutdown = spawn(move || {
            shutdown_entered.send(()).unwrap();
            let result = catch_unwind(AssertUnwindSafe(|| drop(pool)));
            shutdown_done.send(()).unwrap();
            result
        });
        shutdown_arrival.recv().unwrap();
        assert_eq!(shutdown_completion.try_recv(), Err(TryRecvError::Empty));

        release_failure.send(()).unwrap();
        assert!(shutdown.join().unwrap().is_err());
        worker.join().unwrap();
        assert_panics(|| block_on(pending).unwrap());
        assert_eq!(inner.runtime.active_workers(), 0);
    });
}

/// Fire-and-forget capture cleanup releases initialization while joined work is cancelled.
#[test]
fn shutdown_waits_for_initialization_and_cancels_queued_work() {
    with_watchdog(|| {
        let finished = Arc::new(AtomicBool::new(false));
        let (entered, arrival) = channel();
        let (release, wait) = channel();
        let pool = Pool::builder()
            .hardware(hardware())
            .worker_policy(TestPolicy::new(1))
            .thread_init({
                let finished = Arc::clone(&finished);
                let wait = Mutex::new(wait);
                move || {
                    entered.send(()).unwrap();
                    wait.lock().unwrap().recv().unwrap();
                    finished.store(true, Ordering::Release);
                }
            })
            .build();

        let inner = Arc::clone(&pool.inner);
        let capture = OnDrop(Some(Box::new(move || release.send(()).unwrap())));
        let scheduler = pool.scheduler();
        scheduler.spawn_and_forget(move || drop(capture));
        let task = scheduler
            .spawn(|| panic!("queued work must not execute before initialization finishes"));

        arrival.recv().unwrap();
        drop(pool);

        assert!(finished.load(Ordering::Acquire));
        assert_eq!(inner.runtime.active_workers(), 0);
        assert!(!inner.runtime.is_poisoned());
        assert!(block_on(task).unwrap_err().is_cancel());
    });
}
