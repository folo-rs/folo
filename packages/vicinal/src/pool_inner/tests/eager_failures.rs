//! Partial eager construction must join every worker and preserve infrastructure failures.

use std::any::Any;
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread::{self, JoinHandle as ThreadJoinHandle};

use many_cpus::ProcessorSet;
use new_zealand::nz;
use testing::{assert_panics, with_watchdog};

use super::helpers::{OnDrop, PanickingPayload, hardware};
use crate::pool_inner::WorkerEntry;
use crate::{Pool, WorkerPolicy};

/// Failure in another group still owns and joins the first group's worker.
#[test]
fn eager_partial_spawn_failure_joins_workers() {
    with_watchdog(|| {
        let pool = Pool::builder()
            .hardware(hardware())
            .worker_policy(WorkerPolicy::per_memory_region(nz!(1)))
            .build();
        let mut spawned = 0_u32;
        let payload = failed_construction(
            pool,
            1,
            |builder, entry| {
                spawned = spawned.checked_add(1).unwrap();
                if spawned == 2 {
                    Err(io::Error::other("injected eager spawn failure"))
                } else {
                    builder.spawn(entry)
                }
            },
            ProcessorSet::pin_current_thread_to,
        );
        drop(payload);
        assert_eq!(spawned, 2);
    });
}

/// A later affinity failure must also clean up successfully initialized siblings.
#[test]
fn eager_partial_affinity_failure_joins_workers() {
    struct AffinityFailure;

    with_watchdog(|| {
        let pool = Pool::builder()
            .hardware(hardware())
            .worker_policy(WorkerPolicy::per_memory_region(nz!(1)))
            .build();
        let pins = Arc::new(AtomicUsize::new(0));
        let payload = failed_construction(pool, 2, thread::Builder::spawn, move |processors| {
            if pins.fetch_add(1, Ordering::Relaxed) == 1 {
                panic::panic_any(AffinityFailure);
            }
            processors.pin_current_thread_to();
        });
        assert!(payload.is::<AffinityFailure>());
    });
}

/// Joining construction retrieves the first worker panic and contains secondary payload disposal.
#[test]
fn eager_initializer_failures_preserve_the_original_panic() {
    struct InitializationFailure;

    with_watchdog(|| {
        // Both workers reach initialization before either fails, exercising completion reporting
        // separately from affinity readiness and ensuring secondary cleanup has a panic to discard.
        let rendezvous = Barrier::new(2);
        let pool = Pool::builder()
            .hardware(hardware())
            .worker_policy(WorkerPolicy::per_system(nz!(2)))
            .thread_init(move || {
                rendezvous.wait();
                if thread::current().name().unwrap().ends_with("-0") {
                    panic::panic_any(InitializationFailure);
                }
                panic::panic_any(PanickingPayload);
            })
            .build();
        let payload = failed_construction(
            pool,
            2,
            thread::Builder::spawn,
            ProcessorSet::pin_current_thread_to,
        );
        assert!(payload.is::<InitializationFailure>());
    });
}

/// A dropped entry point disconnects startup instead of leaving construction waiting forever.
#[test]
fn eager_worker_exit_before_affinity_is_a_failure() {
    with_watchdog(|| {
        let pool = Pool::builder()
            .hardware(hardware())
            .worker_policy(WorkerPolicy::per_system(nz!(1)))
            .build();
        let inner = Arc::clone(&pool.inner);
        let processors = inner.hardware.all_processors();
        assert_panics(move || {
            let pool = pool;
            pool.inner.start_eager_with(
                processors,
                |builder, entry| builder.spawn(move || drop(entry)),
                ProcessorSet::pin_current_thread_to,
            );
        });
        assert!(inner.worker_handles.lock().unwrap().is_empty());
        assert!(inner.poisoned.load(Ordering::Acquire));
    });
}

/// The public builder must own cleanup before invoking a fallible eager initializer.
#[test]
fn eager_builder_propagates_initializer_failure() {
    struct InitializationFailure;

    with_watchdog(|| {
        let hardware = hardware();
        let processors = hardware.all_processors();
        let payload = panic::catch_unwind(|| {
            Pool::builder()
                .hardware(hardware)
                .worker_policy(WorkerPolicy::per_system(nz!(1)))
                .eager(processors)
                .thread_init(|| panic::panic_any(InitializationFailure))
                .build()
        })
        .unwrap_err();
        assert!(payload.is::<InitializationFailure>());
    });
}

/// Mirrors the builder's owner-before-startup boundary while retaining observations after unwind.
fn failed_construction(
    pool: Pool,
    expected_exits: usize,
    mut spawn: impl FnMut(thread::Builder, WorkerEntry) -> io::Result<ThreadJoinHandle<()>>,
    pin: impl Fn(&ProcessorSet) + Clone + Send + 'static,
) -> Box<dyn Any + Send> {
    let inner = Arc::clone(&pool.inner);
    let scheduler = pool.scheduler();
    let processors = inner.hardware.all_processors();
    let exited = Arc::new(AtomicUsize::new(0));
    let payload = panic::catch_unwind(AssertUnwindSafe({
        let exited = Arc::clone(&exited);
        move || {
            let pool = pool;
            pool.inner.start_eager_with(
                processors,
                |builder, entry| {
                    spawn(
                        builder,
                        Box::new({
                            let exited = Arc::clone(&exited);
                            move || {
                                let _on_exit = OnDrop(Some(Box::new(move || {
                                    exited.fetch_add(1, Ordering::Relaxed);
                                })));
                                entry();
                            }
                        }),
                    )
                },
                pin,
            );
        }
    }))
    .unwrap_err();
    assert_eq!(exited.load(Ordering::Relaxed), expected_exits);
    assert!(inner.worker_handles.lock().unwrap().is_empty());
    assert!(inner.shutdown.load(Ordering::Acquire));
    assert!(inner.poisoned.load(Ordering::Acquire));
    for state in inner.registry.initialized() {
        assert!(state.shutdown_flag.load(Ordering::Acquire));
        assert_eq!(state.get_tasks_spawned(), 0);
    }
    assert_panics(|| scheduler.spawn(|| {}));
    payload
}
