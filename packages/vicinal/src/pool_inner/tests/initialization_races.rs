//! Controlled initializer failures and shutdown at worker-registration boundaries.

use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use futures::executor::block_on;
use many_cpus::ProcessorSet;
use new_zealand::nz;
use testing::{assert_panics, with_watchdog};

use super::helpers::{NoopTask, OnDrop, hardware, pin};
use crate::{DefaultWorkerPolicy, Pool, init_task};

/// Initializer failure cannot lose a sibling's handle while startup still owns registration.
#[test]
fn initialization_failure_during_sibling_startup_joins_every_worker() {
    // A sibling remains to be registered when the first initializer fails.
    const WORKERS: u32 = 2;

    with_watchdog(|| {
        for policy in [
            DefaultWorkerPolicy::per_processor(nz!(WORKERS)),
            DefaultWorkerPolicy::per_memory_region(nz!(WORKERS)),
        ] {
            let hardware = hardware();
            let target = policy.destination(&hardware);
            let initialized = Arc::new(AtomicUsize::new(0));
            let (failed, failure) = mpsc::channel();
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

            assert_eq!(pool.inner.policy.count().get(), WORKERS);
            let inner = Arc::clone(&pool.inner);
            let scheduler = pool.scheduler();
            let state = inner.registry.get_or_init(target.group);
            let slot = state.task_pool.lock().unwrap().alloc_uninit_box();
            let mut spawned = 0_u32;

            // Admission may observe the concurrent failure or finish before poisoning is published.
            let submission = panic::catch_unwind(AssertUnwindSafe(|| {
                inner.enqueue_with(target, state, init_task(slot, NoopTask), false, |handles| {
                    inner.start_workers(
                        target,
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
        let policy = DefaultWorkerPolicy::per_processor(nz!(1));
        let target = policy.destination(&hardware);
        let (initializer_entered, initializer_arrival) = mpsc::channel();
        let (release_initializer, wait_for_release) = mpsc::channel();
        let pool = Pool::builder()
            .hardware(hardware)
            .worker_policy(policy)
            .thread_init({
                let initializer_entered = initializer_entered;
                let wait_for_release = Mutex::new(wait_for_release);
                move || {
                    initializer_entered.send(()).unwrap();
                    wait_for_release.lock().unwrap().recv().unwrap();
                    panic::panic_any(InitializationFailure);
                }
            })
            .build();
        let inner = Arc::clone(&pool.inner);
        let state = inner.registry.get_or_init(target.group);
        inner.install_test_group(target);

        let (failure_entered, failure_arrival) = mpsc::channel();
        let (release_failure, wait_for_failure_release) = mpsc::channel();
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

        inner.start_workers(
            target,
            &mut inner.worker_handles.lock().unwrap(),
            thread::Builder::spawn,
            ProcessorSet::pin_current_thread_to,
        );
        initializer_arrival.recv().unwrap();

        let scheduler = pool.scheduler();
        let pending = scheduler.spawn(|| ());
        release_initializer.send(()).unwrap();
        failure_arrival.recv().unwrap();

        assert!(inner.runtime.is_poisoned());
        assert_eq!(inner.runtime.active_workers(), 1);

        let (shutdown_entered, shutdown_arrival) = mpsc::channel();
        let (shutdown_done, shutdown_completion) = mpsc::channel();
        let shutdown = thread::spawn(move || {
            shutdown_entered.send(()).unwrap();
            drop(pool);
            shutdown_done.send(()).unwrap();
        });
        shutdown_arrival.recv().unwrap();
        assert!(shutdown_completion.try_recv().is_err());

        release_failure.send(()).unwrap();
        shutdown.join().unwrap_err();
        assert_panics(|| block_on(pending).unwrap());
        assert_eq!(inner.runtime.active_workers(), 0);
    });
}

/// Fire-and-forget capture cleanup releases initialization while joined work is cancelled.
#[test]
fn shutdown_waits_for_initialization_and_cancels_queued_work() {
    with_watchdog(|| {
        for policy in [
            DefaultWorkerPolicy::per_processor(nz!(1)),
            DefaultWorkerPolicy::per_memory_region(nz!(1)),
        ] {
            // Pin to the first fake processor so both tasks share one initializer.
            let hardware = hardware();
            pin(&hardware, 0);

            let finished = Arc::new(AtomicBool::new(false));
            let (entered, arrival) = mpsc::channel();
            let (release, wait) = mpsc::channel();
            let pool = Pool::builder()
                .hardware(hardware)
                .worker_policy(policy)
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

            // The fire-and-forget capture releases exactly one initializer.
            assert_eq!(pool.inner.policy.count(), nz!(1));
            let inner = Arc::clone(&pool.inner);
            let capture = OnDrop(Some(Box::new(move || release.send(()).unwrap())));
            let scheduler = pool.scheduler();
            scheduler.spawn_and_forget(move || drop(capture));
            let task = scheduler.spawn(|| {
                panic!("queued work must not execute before initialization finishes");
            });

            arrival.recv().unwrap();
            drop(pool);

            assert!(finished.load(Ordering::Acquire));
            assert_eq!(inner.runtime.active_workers(), 0);
            assert!(!inner.runtime.is_poisoned());
            assert!(block_on(task).unwrap_err().is_cancel());
        }
    });
}
