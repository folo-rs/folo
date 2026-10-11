//! In-process topology, startup and shutdown regression coverage.

use std::collections::BTreeSet;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, mpsc};
use std::{io, thread};

use futures::FutureExt;
use futures::executor::block_on;
use many_cpus::fake::{HardwareBuilder, ProcessorBuilder};
use many_cpus::{Processor, ProcessorSet, SystemHardware};
use new_zealand::nz;
use testing::{assert_panics, with_watchdog};

use crate::{
    ErasedTaskHandle, GroupId, Pool, Scheduler, VicinalTask, WorkerGroup, WorkerPolicy, init_task,
};

/// Sparse regions with multiple eligible CPUs and a quota that excludes some of them.
fn hardware() -> SystemHardware {
    SystemHardware::fake(
        HardwareBuilder::new()
            .processor(ProcessorBuilder::new().id(0).memory_region(2))
            .processor(ProcessorBuilder::new().id(1).memory_region(2))
            .processor(ProcessorBuilder::new().id(3).memory_region(5))
            .max_processor_time(1.0),
    )
}

/// Records the permitted mask, rather than a CPU observed by chance.
fn affinity(hardware: &SystemHardware) -> BTreeSet<u32> {
    hardware
        .thread_processors()
        .unwrap()
        .iter()
        .map(Processor::id)
        .collect()
}

#[test]
fn workers_pin_to_spawning_processors_despite_resource_quota() {
    with_watchdog(|| {
        let hardware = hardware();
        let processors = hardware.all_processors();
        assert!(hardware.processors().len() < processors.len());
        let pool = Pool::builder()
            .hardware(hardware.clone())
            .workers_per_processor(nz!(1))
            .build();
        let scheduler = pool.scheduler();
        let handles = processors.spawn_threads(move |processor| {
            let handle = scheduler.spawn({
                let hardware = hardware.clone();
                move || affinity(&hardware)
            });
            assert_eq!(block_on(handle), BTreeSet::from([processor.id()]));
        });
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(
            pool.inner.worker_handles.lock().unwrap().len(),
            processors.len()
        );
    });
}

#[test]
fn regional_policy_shares_groups_for_every_submission_variant() {
    with_watchdog(|| {
        let hardware = hardware();
        let processors = hardware.all_processors();
        let pool = Pool::builder()
            .hardware(hardware.clone())
            .worker_policy(WorkerPolicy::fixed_per_memory_region(nz!(1)))
            .build();
        let scheduler = pool.scheduler();
        let handles = processors.spawn_threads({
            let processors = processors.clone();
            move |processor| {
                let expected: BTreeSet<_> = processors
                    .iter()
                    .filter(|p| p.memory_region_id() == processor.memory_region_id())
                    .map(Processor::id)
                    .collect();
                let ordinary = scheduler.spawn({
                    let hardware = hardware.clone();
                    move || affinity(&hardware)
                });
                let urgent = scheduler.spawn_urgent({
                    let hardware = hardware.clone();
                    move || affinity(&hardware)
                });
                let (sent, received) = mpsc::channel();
                scheduler.spawn_and_forget({
                    let hardware = hardware.clone();
                    let sent = sent.clone();
                    move || sent.send(affinity(&hardware)).unwrap()
                });
                scheduler.spawn_urgent_and_forget({
                    let hardware = hardware.clone();
                    move || sent.send(affinity(&hardware)).unwrap()
                });
                assert_eq!(block_on(ordinary), expected);
                assert_eq!(block_on(urgent), expected);
                assert_eq!(received.recv().unwrap(), expected);
                assert_eq!(received.recv().unwrap(), expected);
            }
        });
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(pool.inner.registry.initialized().count(), 2);
        assert_eq!(pool.inner.worker_handles.lock().unwrap().len(), 2);
        assert_eq!(
            pool.inner
                .registry
                .get_or_init(GroupId::MemoryRegion(2))
                .get_tasks_spawned(),
            8
        );
        assert_eq!(
            pool.inner
                .registry
                .get_or_init(GroupId::MemoryRegion(5))
                .get_tasks_spawned(),
            4
        );
    });
}

#[test]
fn concurrent_first_submissions_start_exact_configured_count() {
    // More workers than processors proves that intentional oversubscription is retained.
    const WORKERS: u32 = 3;
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
            let destination = if regional { "region-0" } else { "0" };
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
                block_on(submitter.join().unwrap());
            }
            block_on(scheduler.spawn(|| {}));
            assert_eq!(
                pool.inner.worker_handles.lock().unwrap().len(),
                WORKERS as usize
            );
        }
    });
}

#[test]
fn shutdown_before_lazy_state_creation_starts_no_workers() {
    let pool = Pool::builder()
        .hardware(hardware())
        .worker_policy(WorkerPolicy::fixed_per_memory_region(nz!(1)))
        .build();
    let inner = Arc::clone(&pool.inner);
    drop(pool);
    let state = inner.ensure_workers_spawned_with(GroupId::MemoryRegion(2), |_| panic!());
    assert!(state.startup.get().is_none());
    assert!(inner.worker_handles.lock().unwrap().is_empty());
}

#[test]
fn shutdown_accounts_for_in_progress_startup() {
    with_watchdog(|| {
        let pool = Pool::builder()
            .hardware(hardware())
            .workers_per_processor(nz!(1))
            .build();
        let inner = Arc::clone(&pool.inner);
        let (entered, arrivals) = mpsc::channel();
        let (resume, wait) = mpsc::channel();
        let starter = thread::spawn({
            let inner = Arc::clone(&inner);
            move || {
                inner.ensure_workers_spawned_with(GroupId::Processor(0), |handles| {
                    inner.start_workers(
                        GroupId::Processor(0),
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

#[test]
fn partial_thread_creation_failure_is_terminal_and_does_not_poison() {
    with_watchdog(|| {
        let pool = Pool::builder()
            .hardware(hardware())
            .workers_per_processor(nz!(2))
            .build();
        let inner = &pool.inner;
        let destination = GroupId::Processor(0);
        let mut attempts = 0_u32;
        assert_panics(|| {
            inner.ensure_workers_spawned_with(destination, |handles| {
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
        assert_eq!(
            inner.registry.get_or_init(destination).startup.get(),
            Some(&Err(()))
        );
        assert_panics(|| {
            inner.ensure_workers_spawned(destination);
        });
        inner.join_all_workers();
        assert!(inner.worker_handles.lock().unwrap().is_empty());
    });
}

#[test]
fn affinity_failure_is_reported_before_admission() {
    with_watchdog(|| {
        let pool = Pool::builder()
            .hardware(hardware())
            .workers_per_processor(nz!(1))
            .build();
        let inner = &pool.inner;
        let destination = GroupId::Processor(0);
        assert_panics(|| {
            inner.ensure_workers_spawned_with(destination, |handles| {
                inner.start_workers(destination, handles, thread::Builder::spawn, |_| {
                    panic!("injected affinity failure")
                });
            });
        });
        let state = inner.registry.get_or_init(destination);
        assert_eq!(state.startup.get(), Some(&Err(())));
        assert_eq!(state.get_tasks_spawned(), 0);
        assert_panics(|| {
            inner.ensure_workers_spawned(destination);
        });
        inner.join_all_workers();
        assert!(inner.worker_handles.lock().unwrap().is_empty());
    });
}

#[test]
fn empty_region_never_starts_unpinned_workers() {
    let pool = Pool::builder()
        .hardware(hardware())
        .worker_policy(WorkerPolicy::fixed_per_memory_region(nz!(1)))
        .build();
    // The sparse index space contains this ID, but no eligible processor belongs to it.
    assert_panics(|| {
        pool.inner.ensure_workers_spawned(GroupId::MemoryRegion(4));
    });
    assert!(pool.inner.worker_handles.lock().unwrap().is_empty());
}

/// Detects abandoned captures and can re-enter the pool during destruction.
struct OnDrop(Option<Box<dyn FnOnce() + Send>>);

impl Drop for OnDrop {
    fn drop(&mut self) {
        if let Some(callback) = self.0.take() {
            callback();
        }
    }
}

impl VicinalTask for OnDrop {
    fn call(self: Pin<&mut Self>) {
        panic!("an abandoned task must not execute");
    }
}

/// Places a probe between allocation and admission without injecting production-only branches.
fn probe(state: &WorkerGroup, callback: impl FnOnce() + Send + 'static) -> ErasedTaskHandle {
    let slot = state.task_pool.lock().unwrap().alloc_uninit_box();
    init_task(slot, OnDrop(Some(Box::new(callback))))
}

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
    inner.enqueue(destination, state, task, false);
    assert_eq!(dropped.load(Ordering::Relaxed), 1);
    assert!(state.regular_queue.lock().unwrap().is_empty());
}

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

/// All variants abandon synchronously after the owner stops the pool.
fn assert_retained_scheduler(scheduler: &Scheduler) {
    assert_panics(|| block_on(scheduler.spawn(|| panic!())));
    assert_panics(|| block_on(scheduler.spawn_urgent(|| panic!())));
    let dropped = Arc::new(AtomicUsize::new(0));
    for urgent in [false, true] {
        let capture = OnDrop(Some(Box::new({
            let dropped = Arc::clone(&dropped);
            move || {
                dropped.fetch_add(1, Ordering::Relaxed);
            }
        })));
        let task = move || drop(capture);
        if urgent {
            scheduler.spawn_urgent_and_forget(task);
        } else {
            scheduler.spawn_and_forget(task);
        }
    }
    assert_eq!(dropped.load(Ordering::Relaxed), 2);
}

#[test]
fn separate_enqueues_notify_separate_waiters() {
    let pool = Pool::builder().hardware(hardware()).build();
    let destination = GroupId::Processor(0);
    let state = pool.inner.registry.get_or_init(destination);
    let first = state.wake_event.listen();
    let second = state.wake_event.listen();
    pool.inner
        .enqueue(destination, state, probe(state, || {}), false);
    pool.inner
        .enqueue(destination, state, probe(state, || {}), true);
    assert_eq!(first.now_or_never(), Some(()));
    assert_eq!(second.now_or_never(), Some(()));
}
