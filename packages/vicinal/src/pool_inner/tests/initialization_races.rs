//! Controlled initializer failures and shutdown at worker-registration boundaries.

use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use futures::executor::block_on;
use many_cpus::ProcessorSet;
use new_zealand::nz;
use testing::{assert_panics, with_watchdog};

use super::helpers::{NoopTask, OnDrop, hardware};
use crate::{Pool, WorkerPolicy, init_task};

/// Initializer failure cannot lose a sibling's handle while startup still owns registration.
#[test]
fn initialization_failure_during_sibling_startup_joins_every_worker() {
    // A sibling remains to be registered when the first initializer fails.
    const WORKERS: u32 = 2;
    with_watchdog(|| {
        for policy in [
            WorkerPolicy::fixed_per_processor(nz!(WORKERS)),
            WorkerPolicy::fixed_per_memory_region(nz!(WORKERS)),
        ] {
            let initialized = Arc::new(AtomicUsize::new(0));
            let (failed, failure) = mpsc::channel();
            let pool = Pool::builder()
                .hardware(hardware())
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
            let target = inner.policy.destination(&inner.hardware);
            let state = inner.registry.get_or_init(target.group);
            let slot = state.task_pool.lock().unwrap().alloc_uninit_box();
            let mut spawned = 0_u32;
            // Admission may observe the concurrent failure or finish before poisoning is published.
            let _submission = panic::catch_unwind(AssertUnwindSafe(|| {
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
            assert_eq!(inner.worker_handles.lock().unwrap().len(), WORKERS as usize);
            assert_panics(|| drop(pool));
            assert_eq!(initialized.load(Ordering::Relaxed), WORKERS as usize);
            assert!(inner.worker_handles.lock().unwrap().is_empty());
            assert!(state.task_pool.lock().unwrap().is_empty());
            assert!(inner.poisoned.load(Ordering::Acquire));
            assert_panics(|| scheduler.spawn(|| {}));
        }
    });
}

/// Abandoning queued captures releases initialization before shutdown joins its worker.
#[test]
fn shutdown_waits_for_initialization_and_abandons_queued_work() {
    with_watchdog(|| {
        for policy in [
            WorkerPolicy::fixed_per_processor(nz!(1)),
            WorkerPolicy::fixed_per_memory_region(nz!(1)),
        ] {
            let finished = Arc::new(AtomicBool::new(false));
            let (entered, arrival) = mpsc::channel();
            let (release, wait) = mpsc::channel();
            let pool = Pool::builder()
                .hardware(hardware())
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
            // The abandoned capture releases exactly one initializer.
            assert_eq!(pool.inner.policy.count(), nz!(1));
            let inner = Arc::clone(&pool.inner);
            let capture = OnDrop(Some(Box::new(move || release.send(()).unwrap())));
            let task = pool.scheduler().spawn(move || {
                drop(capture);
                panic!("queued work must not execute before initialization finishes");
            });
            arrival.recv().unwrap();
            drop(pool);
            assert!(finished.load(Ordering::Acquire));
            assert!(inner.worker_handles.lock().unwrap().is_empty());
            assert!(!inner.poisoned.load(Ordering::Acquire));
            assert_panics(|| block_on(task));
        }
    });
}
