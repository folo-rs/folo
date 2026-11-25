//! Worker readiness, exact counts and registration through startup races.

use std::collections::BTreeSet;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind, panic_any};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Barrier, Mutex, OnceLock};
use std::thread::{Builder, current, spawn};

use futures::executor::block_on;
use many_cpus::fake::HardwareBuilder;
use many_cpus::{ProcessorSet, SystemHardware};
use new_zealand::nz;
use testing::{assert_panics, with_watchdog};

use super::helpers::{OnDrop, PanickingPayload, TestPolicy, affinity, hardware, probe};
use crate::{
    BuiltInWorkerPolicy, GroupId, PolicyKind, Pool, PrepareContext, PrepareError, SubmissionTarget,
    WorkerGroupId, WorkerGroups, WorkerPolicy,
};

/// Pauses policy selection after a test worker reaches the ordinary participation boundary.
struct GatedSelectionPolicy {
    inner: TestPolicy,
    worker_started: OnceLock<()>,
    entered: Sender<()>,
    exited: Sender<()>,
    release: Mutex<Receiver<()>>,
}

impl WorkerPolicy for GatedSelectionPolicy {
    fn prepare(&mut self, context: PrepareContext<'_>) -> Result<(), PrepareError> {
        self.inner.prepare(context)
    }

    fn select_group(&self, groups: &WorkerGroups) -> WorkerGroupId {
        let id = self.inner.select_group(groups);
        self.worker_started.get_or_init(|| {
            let group = self.inner.group();
            _ = spawn({
                let worker = group.add_worker();
                let exited = self.exited.clone();
                move || {
                    worker.run();
                    exited.send(()).unwrap();
                }
            });
            group.wait_for_initialized_workers(nz!(1_usize));
        });
        self.entered.send(()).unwrap();
        self.release.lock().unwrap().recv().unwrap();
        id
    }
}

/// Fails exactly at the custom policy boundary after pool preparation.
struct PanickingSelectionPolicy {
    inner: TestPolicy,
}

impl WorkerPolicy for PanickingSelectionPolicy {
    fn prepare(&mut self, context: PrepareContext<'_>) -> Result<(), PrepareError> {
        self.inner.prepare(context)
    }

    fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
        panic_any(StartupFailure);
    }
}

struct StartupFailure;

/// Processor-local startup must serialize concurrent first submitters.
#[test]
fn concurrent_processor_submissions_start_exact_configured_count() {
    assert_concurrent_startup(PolicyKind::Processor);
}

/// A shared region starts one worker group despite concurrent first submitters.
#[test]
fn concurrent_regional_submissions_start_exact_configured_count() {
    assert_concurrent_startup(PolicyKind::MemoryRegion);
}

/// System-wide startup must not multiply capacity for concurrent first submitters.
#[test]
fn concurrent_system_submissions_start_exact_configured_count() {
    assert_concurrent_startup(PolicyKind::System);
}

/// Each policy gets a bounded Miri case with every worker occupied to expose exact capacity.
fn assert_concurrent_startup(kind: PolicyKind) {
    // More workers than processors proves that intentional oversubscription is retained.
    const WORKERS: u32 = 2;

    with_watchdog(move || {
        let (policy, destination) = match kind {
            PolicyKind::Processor => (BuiltInWorkerPolicy::per_processor(nz!(WORKERS)), "p0"),
            PolicyKind::MemoryRegion => {
                (BuiltInWorkerPolicy::per_memory_region(nz!(WORKERS)), "r0")
            }
            PolicyKind::System => (BuiltInWorkerPolicy::per_system(nz!(WORKERS)), "s"),
        };
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
        let pool = Pool::builder()
            .name("workers")
            .hardware(hardware.clone())
            .worker_policy(policy)
            .build();

        let scheduler = pool.scheduler();
        let start = Arc::new(Barrier::new(WORKERS as usize));
        let (entered, arrivals) = channel();
        let mut submitters = Vec::new();
        let mut releases = Vec::new();

        for _ in 0..WORKERS {
            let (release, wait) = channel();
            releases.push(release);
            submitters.push(spawn({
                let start = Arc::clone(&start);
                let scheduler = scheduler.clone();
                let entered = entered.clone();
                let hardware = hardware.clone();
                move || {
                    start.wait();

                    scheduler.spawn(move || {
                        entered
                            .send((current().name().unwrap().to_owned(), affinity(&hardware)))
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

        assert_eq!(
            names,
            (0..WORKERS)
                .map(|i| format!("workers-{destination}-{i}"))
                .collect()
        );
        let worker_count = pool.inner.runtime.active_workers();
        assert_eq!(worker_count, WORKERS as usize);

        for release in releases {
            release.send(()).unwrap();
        }
        for submitter in submitters {
            block_on(submitter.join().unwrap()).unwrap();
        }

        block_on(scheduler.spawn(|| {})).unwrap();
        let worker_count = pool.inner.runtime.active_workers();
        assert_eq!(worker_count, WORKERS as usize);
    });
}

/// Pauses startup while it owns registration, forcing shutdown to account for those handles.
#[test]
fn shutdown_accounts_for_in_progress_startup() {
    with_watchdog(|| {
        let (entered, selection_entered) = channel();
        let (exited, worker_exited) = channel();
        let (release, selection_release) = channel();
        let policy = TestPolicy::new(0);
        let pool = Pool::builder()
            .hardware(hardware())
            .worker_policy(GatedSelectionPolicy {
                inner: policy.clone(),
                worker_started: OnceLock::new(),
                entered,
                exited,
                release: Mutex::new(selection_release),
            })
            .build();
        let inner = Arc::clone(&pool.inner);
        let scheduler = pool.scheduler();
        let group = policy.group();
        let state = group.state();
        let (worker_waiting, worker_wait) = channel();
        let (release_worker, worker_release) = channel();
        let worker_release = Mutex::new(Some(worker_release));
        assert!(
            state
                .before_wait
                .set(Box::new(move |_, _| {
                    worker_waiting.send(()).unwrap();
                    let worker_release = worker_release.lock().unwrap().take().unwrap();
                    worker_release.recv().unwrap();
                }))
                .is_ok()
        );
        let starter = spawn(move || block_on(scheduler.spawn(|| 42)).unwrap_err());

        selection_entered.recv().unwrap();
        worker_wait.recv().unwrap();
        let (shutdown_waiting, shutdown_wait) = channel();
        assert!(
            inner
                .runtime
                .before_worker_wait
                .set(Box::new(move || {
                    shutdown_waiting.send(()).unwrap();
                }))
                .is_ok()
        );
        let (shutdown_done, shutdown_completion) = channel();
        let shutdown = spawn(move || {
            drop(pool);
            shutdown_done.send(()).unwrap();
        });
        shutdown_wait.recv().unwrap();
        assert_eq!(shutdown_completion.try_recv(), Err(TryRecvError::Empty));

        release_worker.send(()).unwrap();
        worker_exited.recv().unwrap();
        shutdown.join().unwrap();
        release.send(()).unwrap();
        assert!(starter.join().unwrap().is_cancel());

        assert!(inner.runtime.is_shutdown());
        assert_eq!(inner.runtime.active_workers(), 0);
    });
}

/// Failing after one successful spawn must retain that handle without poisoning shared locks.
#[test]
fn partial_thread_creation_failure_does_not_poison_locks() {
    with_watchdog(|| {
        let pool = Pool::builder()
            .hardware(hardware())
            .worker_policy(BuiltInWorkerPolicy::per_processor(nz!(2)))
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
                    nz!(2),
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
        inner.runtime.wait_for_workers();
        assert_eq!(inner.runtime.active_workers(), 0);
        assert_eq!(state.startup.get(), Some(&Err(())));

        assert_panics(|| {
            inner.enqueue(state, probe(state, || {}), false);
        });

        inner.shutdown_and_wait_for_workers();
        assert_eq!(inner.runtime.active_workers(), 0);
    });
}

/// A spawned thread is not ready until affinity setup succeeds and acknowledges readiness.
#[test]
fn affinity_failure_is_reported_before_admission() {
    with_watchdog(|| {
        let initialized = Arc::new(AtomicBool::new(false));
        let pool = Pool::builder()
            .hardware(hardware())
            .worker_policy(BuiltInWorkerPolicy::per_processor(nz!(1)))
            .thread_init({
                let initialized = Arc::clone(&initialized);
                move || initialized.store(true, Ordering::Relaxed)
            })
            .build();
        let inner = &pool.inner;
        let destination = SubmissionTarget {
            group: GroupId::Processor(0),
            processor: 0,
        };
        let state = inner.registry.get_or_init(destination.group);

        assert_panics(|| {
            inner.enqueue_with(destination, state, probe(state, || {}), false, |handles| {
                inner.start_workers(destination, nz!(1), handles, Builder::spawn, |_| {
                    panic!("injected affinity failure")
                });
            });
        });

        assert_eq!(state.startup.get(), Some(&Err(())));
        assert_eq!(state.get_tasks_spawned(), 0);

        assert_panics(|| {
            inner.enqueue(state, probe(state, || {}), false);
        });

        inner.shutdown_and_wait_for_workers();
        assert_eq!(inner.runtime.active_workers(), 0);
        assert!(!initialized.load(Ordering::Relaxed));
    });
}

/// Unadmitted captures stay outside startup's unwind and later poisoned fast-path rejection.
#[test]
fn startup_failure_contains_unadmitted_capture_destruction() {
    let policy = TestPolicy::new(0);
    let pool = Pool::builder()
        .hardware(hardware())
        .worker_policy(PanickingSelectionPolicy {
            inner: policy.clone(),
        })
        .build();
    let inner = &pool.inner;
    let group = policy.group();
    let state = group.state();
    let scheduler = pool.scheduler();

    let payload = catch_unwind(AssertUnwindSafe(|| {
        let capture = OnDrop(Some(Box::new(|| panic_any(PanickingPayload))));
        scheduler.spawn_and_forget(move || drop(capture));
    }))
    .unwrap_err();

    assert!(payload.is::<StartupFailure>());
    assert!(inner.runtime.is_poisoned());
    assert!(state.task_pool.lock().unwrap().is_empty());

    let executed = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicBool::new(false));
    let payload = catch_unwind(AssertUnwindSafe({
        let executed = Arc::clone(&executed);
        let dropped = Arc::clone(&dropped);
        move || {
            let capture = OnDrop(Some(Box::new(move || {
                dropped.store(true, Ordering::Release);
                panic_any(PanickingPayload);
            })));
            _ = scheduler.spawn(move || {
                executed.store(true, Ordering::Release);
                drop(capture);
            });
        }
    }))
    .unwrap_err();

    _ = payload;
    assert!(dropped.load(Ordering::Acquire));
    assert!(!executed.load(Ordering::Acquire));
    assert!(state.task_pool.lock().unwrap().is_empty());
    assert_eq!(inner.runtime.active_workers(), 0);
}

/// Sparse topology holes must fail instead of selecting an unrelated processor.
#[test]
fn empty_region_never_starts_unpinned_workers() {
    let pool = Pool::builder()
        .hardware(hardware())
        .worker_policy(BuiltInWorkerPolicy::per_memory_region(nz!(1)))
        .build();

    // The sparse index space contains this ID, but no eligible processor belongs to it.
    assert_panics(|| {
        let target = SubmissionTarget {
            group: GroupId::MemoryRegion(4),
            processor: 0,
        };
        _ = pool.inner.registry.get_or_init(target.group);
        pool.inner.start_workers(
            target,
            nz!(1),
            &mut Vec::new(),
            Builder::spawn,
            ProcessorSet::pin_current_thread_to,
        );
    });
    assert_eq!(pool.inner.runtime.active_workers(), 0);
}

/// Queue abandonment in another group releases its blocked worker during startup poisoning.
#[test]
fn startup_failure_stops_every_group() {
    with_watchdog(|| {
        for policy in [
            BuiltInWorkerPolicy::per_processor(nz!(1)),
            BuiltInWorkerPolicy::per_memory_region(nz!(1)),
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
            let (entered, arrivals) = channel();
            let (release, wait) = channel();

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
