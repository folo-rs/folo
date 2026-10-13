//! Worker readiness, exact counts and registration through startup races.

use std::collections::BTreeSet;
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Barrier, mpsc};
use std::{io, thread};

use futures::executor::block_on;
use many_cpus::fake::HardwareBuilder;
use many_cpus::{ProcessorSet, SystemHardware};
use new_zealand::nz;
use testing::{assert_panics, with_watchdog};

use super::helpers::{NoopTask, OnDrop, PanickingPayload, affinity, hardware, probe};
use crate::{GroupId, Pool, SubmissionTarget, WorkerPolicy, init_task};

/// Simultaneous first submitters block every worker so exact startup capacity is observable.
#[test]
fn concurrent_first_submissions_start_exact_configured_count() {
    // More workers than processors proves that intentional oversubscription is retained.
    const WORKERS: u32 = 2;
    with_watchdog(|| {
        for regional in [false, true] {
            let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
            let policy = if regional {
                WorkerPolicy::fixed_per_memory_region(nz!(WORKERS))
            } else {
                WorkerPolicy::fixed_per_processor(nz!(WORKERS))
            };
            let pool = Pool::builder()
                .name("workers")
                .hardware(hardware.clone())
                .worker_policy(policy)
                .build();
            let scheduler = pool.scheduler();
            let start = Arc::new(Barrier::new(WORKERS as usize));
            let (entered, arrivals) = mpsc::channel();
            let mut submitters = Vec::new();
            let mut releases = Vec::new();
            for _ in 0..WORKERS {
                let (release, wait) = mpsc::channel();
                releases.push(release);
                submitters.push(thread::spawn({
                    let start = Arc::clone(&start);
                    let scheduler = scheduler.clone();
                    let entered = entered.clone();
                    let hardware = hardware.clone();
                    move || {
                        start.wait();
                        scheduler.spawn(move || {
                            entered
                                .send((
                                    thread::current().name().unwrap().to_owned(),
                                    affinity(&hardware),
                                ))
                                .unwrap();
                            wait.recv().unwrap();
                        })
                    }
                }));
            }
            drop(entered);
            let mut names = BTreeSet::new();
            for _ in 0..WORKERS {
                let (name, mask) = arrivals.recv().unwrap();
                assert_eq!(mask, BTreeSet::from([0]));
                assert!(names.insert(name));
            }
            let destination = if regional { "r0" } else { "p0" };
            assert_eq!(
                names,
                (0..WORKERS)
                    .map(|i| format!("workers-{destination}-{i}"))
                    .collect()
            );
            assert_eq!(
                pool.inner.worker_handles.lock().unwrap().len(),
                WORKERS as usize
            );
            for release in releases {
                release.send(()).unwrap();
            }
            for submitter in submitters {
                block_on(submitter.join().unwrap()).unwrap();
            }
            block_on(scheduler.spawn(|| {})).unwrap();
            assert_eq!(
                pool.inner.worker_handles.lock().unwrap().len(),
                WORKERS as usize
            );
        }
    });
}

/// Pauses startup while it owns registration, forcing shutdown to account for those handles.
#[test]
fn shutdown_accounts_for_in_progress_startup() {
    with_watchdog(|| {
        let pool = Pool::builder()
            .hardware(hardware())
            .worker_policy(WorkerPolicy::fixed_per_processor(nz!(1)))
            .build();
        let inner = Arc::clone(&pool.inner);
        let (entered, arrivals) = mpsc::channel();
        let (resume, wait) = mpsc::channel();
        let starter = thread::spawn({
            let inner = Arc::clone(&inner);
            move || {
                let target = SubmissionTarget {
                    group: GroupId::Processor(0),
                    processor: 0,
                };
                let state = inner.registry.get_or_init(target.group);
                let slot = state.task_pool.lock().unwrap().alloc_uninit_box();
                let task = init_task(slot, NoopTask);
                inner.enqueue_with(target, state, task, false, |handles| {
                    inner.start_workers(
                        target,
                        handles,
                        thread::Builder::spawn,
                        ProcessorSet::pin_current_thread_to,
                    );
                    entered.send(()).unwrap();
                    wait.recv().unwrap();
                });
            }
        });
        arrivals.recv().unwrap();
        // Startup retains the registration lock until every handle is accounted for.
        _ = inner.worker_handles.try_lock().unwrap_err();
        let shutdown = thread::spawn(move || drop(pool));
        resume.send(()).unwrap();
        starter.join().unwrap();
        shutdown.join().unwrap();
        assert!(inner.shutdown.load(Ordering::Acquire));
        assert!(inner.worker_handles.lock().unwrap().is_empty());
        assert!(
            inner
                .registry
                .get_or_init(GroupId::Processor(0))
                .shutdown_flag
                .load(Ordering::Acquire)
        );
    });
}

/// Failing after one successful spawn must retain that handle without poisoning shared locks.
#[test]
fn partial_thread_creation_failure_does_not_poison_locks() {
    with_watchdog(|| {
        let pool = Pool::builder()
            .hardware(hardware())
            .worker_policy(WorkerPolicy::fixed_per_processor(nz!(2)))
            .build();
        let inner = &pool.inner;
        let destination = SubmissionTarget {
            group: GroupId::Processor(0),
            processor: 0,
        };
        let state = inner.registry.get_or_init(destination.group);
        let mut attempts = 0_u32;
        assert_panics(|| {
            inner.enqueue_with(destination, state, probe(state, || {}), false, |handles| {
                inner.start_workers(
                    destination,
                    handles,
                    |builder, entry| {
                        attempts = attempts.checked_add(1).unwrap();
                        if attempts == 2 {
                            Err(io::Error::other("injected thread creation failure"))
                        } else {
                            builder.spawn(entry)
                        }
                    },
                    ProcessorSet::pin_current_thread_to,
                );
            });
        });
        assert_eq!(attempts, 2);
        assert_eq!(inner.worker_handles.lock().unwrap().len(), 1);
        assert_eq!(state.startup.get(), Some(&Err(())));
        assert_panics(|| {
            inner.enqueue(destination, state, probe(state, || {}), false);
        });
        inner.join_all_workers();
        assert!(inner.worker_handles.lock().unwrap().is_empty());
    });
}

/// A spawned thread is not ready until affinity setup succeeds and acknowledges readiness.
#[test]
fn affinity_failure_is_reported_before_admission() {
    with_watchdog(|| {
        let pool = Pool::builder()
            .hardware(hardware())
            .worker_policy(WorkerPolicy::fixed_per_processor(nz!(1)))
            .build();
        let inner = &pool.inner;
        let destination = SubmissionTarget {
            group: GroupId::Processor(0),
            processor: 0,
        };
        let state = inner.registry.get_or_init(destination.group);
        assert_panics(|| {
            inner.enqueue_with(destination, state, probe(state, || {}), false, |handles| {
                inner.start_workers(destination, handles, thread::Builder::spawn, |_| {
                    panic!("injected affinity failure")
                });
            });
        });
        assert_eq!(state.startup.get(), Some(&Err(())));
        assert_eq!(state.get_tasks_spawned(), 0);
        assert_panics(|| {
            inner.enqueue(destination, state, probe(state, || {}), false);
        });
        inner.join_all_workers();
        assert!(inner.worker_handles.lock().unwrap().is_empty());
    });
}

/// Unadmitted captures stay outside startup's unwind and later poisoned fast-path rejection.
#[test]
fn startup_failure_contains_unadmitted_capture_destruction() {
    struct StartupFailure;

    let pool = Pool::builder().hardware(hardware()).build();
    let inner = &pool.inner;
    let target = SubmissionTarget {
        group: GroupId::Processor(0),
        processor: 0,
    };
    let state = inner.registry.get_or_init(target.group);
    let payload = panic::catch_unwind(AssertUnwindSafe(|| {
        inner.enqueue_with(
            target,
            state,
            probe(state, || panic::panic_any(PanickingPayload)),
            false,
            |_| panic::panic_any(StartupFailure),
        );
    }))
    .unwrap_err();
    assert!(payload.is::<StartupFailure>());
    assert!(inner.poisoned.load(Ordering::Acquire));
    assert!(state.task_pool.lock().unwrap().is_empty());
    assert_panics(|| {
        inner.enqueue(
            target,
            state,
            probe(state, || panic::panic_any(PanickingPayload)),
            true,
        );
    });
    assert!(state.task_pool.lock().unwrap().is_empty());
    assert!(inner.worker_handles.lock().unwrap().is_empty());
}

/// Sparse topology holes must fail instead of selecting an unrelated processor.
#[test]
fn empty_region_never_starts_unpinned_workers() {
    let pool = Pool::builder()
        .hardware(hardware())
        .worker_policy(WorkerPolicy::fixed_per_memory_region(nz!(1)))
        .build();
    // The sparse index space contains this ID, but no eligible processor belongs to it.
    assert_panics(|| {
        let target = SubmissionTarget {
            group: GroupId::MemoryRegion(4),
            processor: 0,
        };
        let state = pool.inner.registry.get_or_init(target.group);
        pool.inner
            .enqueue(target, state, probe(state, || {}), false);
    });
    assert!(pool.inner.worker_handles.lock().unwrap().is_empty());
}

/// Queue abandonment in another group releases its blocked worker during startup poisoning.
#[test]
fn startup_failure_stops_every_group() {
    with_watchdog(|| {
        for policy in [
            WorkerPolicy::fixed_per_processor(nz!(1)),
            WorkerPolicy::fixed_per_memory_region(nz!(1)),
        ] {
            let hardware = hardware();
            let other_region = hardware
                .all_processors()
                .to_builder()
                .filter(|processor| processor.id() == 3)
                .take_all()
                .unwrap();
            other_region.pin_current_thread_to();
            let pool = Pool::builder()
                .hardware(hardware.clone())
                .worker_policy(policy.clone())
                .build();
            let scheduler = pool.scheduler();
            let (entered, arrivals) = mpsc::channel();
            let (release, wait) = mpsc::channel();
            let running = scheduler.spawn(move || {
                entered.send(()).unwrap();
                wait.recv().unwrap();
            });
            arrivals.recv().unwrap();
            let capture = OnDrop(Some(Box::new({
                let scheduler = scheduler.clone();
                move || {
                    assert_panics(|| scheduler.spawn(|| {}));
                    release.send(()).unwrap();
                }
            })));
            let abandoned = scheduler.spawn(move || drop(capture));

            hardware
                .all_processors()
                .to_builder()
                .filter(|processor| processor.id() == 0)
                .take_all()
                .unwrap()
                .pin_current_thread_to();
            let target = policy.destination(&hardware);
            let state = pool.inner.registry.get_or_init(target.group);
            assert_panics(|| {
                pool.inner
                    .enqueue_with(target, state, probe(state, || {}), false, |_| {
                        panic!("injected startup failure")
                    });
            });
            let error = block_on(abandoned).unwrap_err();
            assert!(error.is_cancel());
            // Disposing of the retained body releases the already-running task.
            drop(error);
            block_on(running).unwrap();
            for state in pool.inner.registry.initialized() {
                assert!(state.shutdown_flag.load(Ordering::Acquire));
            }
            other_region.pin_current_thread_to();
            assert_panics(|| scheduler.spawn(|| {}));
        }
    });
}
