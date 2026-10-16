//! Eager counts, initial masks, callback completion and subsequent lazy expansion.

use std::cell::Cell;
use std::collections::{BTreeSet, HashSet};
use std::num::NonZero;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex, OnceLock, mpsc};
use std::{iter, thread};

use futures::executor::block_on;
use many_cpus::SystemHardware;
use many_cpus::fake::HardwareBuilder;
use new_zealand::nz;
use testing::{assert_panics, with_watchdog};

use super::helpers::{affinity, assert_retained_scheduler, hardware, pin, regional_hardware};
use crate::{GroupId, PolicyKind, Pool, Scheduler, WorkerGroup, WorkerPolicy};

thread_local! {
    static INITIALIZED: Cell<bool> = const { Cell::new(false) };
}

#[test]
fn eager_processor_workers_are_ready_without_submissions() {
    assert_ready(PolicyKind::Processor);
}

#[test]
fn eager_regional_workers_are_ready_without_submissions() {
    assert_ready(PolicyKind::MemoryRegion);
}

#[test]
fn eager_system_workers_are_ready_without_submissions() {
    assert_ready(PolicyKind::System);
}

/// Includes duplicate entries, idle siblings, sparse regions and a strict subset of one region.
fn assert_ready(kind: PolicyKind) {
    // Siblings expose partial initialization; selected CPUs exceed the fake hardware's quota.
    const WORKERS: NonZero<u32> = nz!(2);
    with_watchdog(move || {
        let hardware = regional_hardware();
        pin(&hardware, 1);
        let processors = hardware
            .all_processors()
            .filter(|processor| [0, 3, 8].contains(&processor.id()))
            .unwrap();
        // take_exact preserves repeated entries, but they still identify the same destinations.
        let mut selection = processors.processors().clone();
        selection.push(selection.first().clone());
        let processors = processors.to_builder().take_exact(selection);
        let observations = Arc::new(Mutex::new(Vec::new()));
        let pool = Pool::builder()
            .name("prestarted")
            .eager(processors)
            .hardware(hardware.clone())
            .worker_policy(policy(kind, WORKERS))
            .thread_init({
                let hardware = hardware.clone();
                let observations = Arc::clone(&observations);
                move || {
                    assert!(!INITIALIZED.replace(true));
                    assert!(thread::current().name().unwrap().contains("prestarted"));
                    observations
                        .lock()
                        .unwrap()
                        .push((thread::current().id(), affinity(&hardware)));
                }
            })
            .build();

        let expected_masks = match kind {
            PolicyKind::Processor => vec![
                BTreeSet::from([0]),
                BTreeSet::from([3]),
                BTreeSet::from([8]),
            ],
            PolicyKind::MemoryRegion => vec![BTreeSet::from([0, 3]), BTreeSet::from([8])],
            PolicyKind::System => vec![BTreeSet::from([0, 3, 8])],
        };
        let group_count = expected_masks.len();
        let mut expected: Vec<_> = expected_masks
            .into_iter()
            .flat_map(|mask| iter::repeat_n(mask, WORKERS.get() as usize))
            .collect();
        expected.sort();
        let observations = observations.lock().unwrap().clone();
        let threads: HashSet<_> = observations.iter().map(|(id, _)| *id).collect();
        let mut masks: Vec<_> = observations.into_iter().map(|(_, mask)| mask).collect();
        masks.sort();
        assert_eq!(masks, expected);
        assert_eq!(threads.len(), expected.len());
        let worker_count = pool.inner.worker_handles.lock().unwrap().len();
        assert_eq!(worker_count, expected.len());
        assert_eq!(pool.inner.registry.initialized().count(), group_count);
        assert_eq!(affinity(&hardware), BTreeSet::from([1]));
        assert!(!INITIALIZED.get());
        for state in pool.inner.registry.initialized() {
            assert_eq!(state.startup.get(), Some(&Ok(())));
            assert_eq!(state.get_tasks_spawned(), 0);
            assert!(state.regular_queue.lock().unwrap().is_empty());
            assert!(state.urgent_queue.lock().unwrap().is_empty());
            assert!(state.task_pool.lock().unwrap().is_empty());
        }

        let scheduler = pool.scheduler();
        for processor in [0, 3, 8] {
            pin(&hardware, processor);
            let worker = block_on(scheduler.spawn(|| {
                assert!(INITIALIZED.get());
                thread::current().id()
            }))
            .unwrap();
            assert!(threads.contains(&worker));
        }
        let worker_count = pool.inner.worker_handles.lock().unwrap().len();
        assert_eq!(worker_count, expected.len());
        for state in pool.inner.registry.initialized() {
            let expected_revision = usize::from(!matches!(kind, PolicyKind::Processor));
            assert_eq!(
                state.affinity_revision.load(Ordering::Acquire),
                expected_revision
            );
        }
        drop(pool);
        assert_retained_scheduler(&scheduler);
    });
}

#[test]
fn eager_processor_selection_can_grow_lazily() {
    assert_expansion(PolicyKind::Processor);
}

#[test]
fn eager_regional_selection_can_grow_lazily() {
    assert_expansion(PolicyKind::MemoryRegion);
}

#[test]
fn eager_system_selection_can_grow_lazily() {
    assert_expansion(PolicyKind::System);
}

/// Every submission variant uses seeded membership, without forgetting it during expansion.
fn assert_expansion(kind: PolicyKind) {
    with_watchdog(move || {
        let hardware = regional_hardware();
        let processors = hardware
            .all_processors()
            .filter(|processor| [0, 3].contains(&processor.id()))
            .unwrap();
        // Eager completion and task messages publish these increments, not the counter itself.
        let initialized = Arc::new(AtomicUsize::new(0));
        let pool = Pool::builder()
            .hardware(hardware.clone())
            .worker_policy(policy(kind, nz!(1)))
            .eager(processors)
            .thread_init({
                let initialized = Arc::clone(&initialized);
                move || {
                    assert!(!INITIALIZED.replace(true));
                    initialized.fetch_add(1, Ordering::Relaxed);
                }
            })
            .build();
        let scheduler = pool.scheduler();
        let mut members = BTreeSet::from([0, 3]);
        for (variant, processor) in [0, 3, 2, 8].into_iter().enumerate() {
            pin(&hardware, processor);
            members.insert(processor);
            let expected = match kind {
                PolicyKind::Processor => BTreeSet::from([processor]),
                PolicyKind::MemoryRegion if processor == 8 => BTreeSet::from([8]),
                PolicyKind::MemoryRegion | PolicyKind::System => members.clone(),
            };
            let (sent, received) = mpsc::channel();
            let task = {
                let hardware = hardware.clone();
                move || {
                    assert!(INITIALIZED.get());
                    sent.send(affinity(&hardware)).unwrap();
                }
            };
            match variant {
                0 => block_on(scheduler.spawn(task)).unwrap(),
                1 => block_on(scheduler.spawn_urgent(task)).unwrap(),
                2 => scheduler.spawn_and_forget(task),
                3 => scheduler.spawn_urgent_and_forget(task),
                _ => unreachable!(),
            }
            assert_eq!(received.recv().unwrap(), expected);
            let expected_count = match kind {
                PolicyKind::Processor => members.len(),
                PolicyKind::MemoryRegion => {
                    if members.contains(&8) {
                        2
                    } else {
                        1
                    }
                }
                PolicyKind::System => 1,
            };
            let worker_count = pool.inner.worker_handles.lock().unwrap().len();
            assert_eq!(worker_count, expected_count);
            assert_eq!(initialized.load(Ordering::Relaxed), expected_count);
        }
        assert_eq!(
            pool.inner
                .registry
                .initialized()
                .map(WorkerGroup::get_tasks_spawned)
                .sum::<u64>(),
            4
        );
    });
}

/// The final policy, hardware, callback and eager selection apply regardless of setter order.
#[test]
fn eager_setters_replace_configuration_without_starting_workers() {
    with_watchdog(|| {
        let hardware = hardware();
        // Setters start no workers; build's completion handshake publishes callback increments.
        let initialized = Arc::new(AtomicUsize::new(0));
        let builder = Pool::builder()
            .hardware(SystemHardware::fake(HardwareBuilder::from_counts(
                nz!(1),
                nz!(1),
            )))
            .eager(hardware.all_processors())
            .worker_policy(WorkerPolicy::per_processor(nz!(2)))
            .thread_init(|| panic!("replaced callback"))
            .eager(
                hardware
                    .all_processors()
                    .filter(|processor| processor.id() == 3)
                    .unwrap(),
            )
            .worker_policy(WorkerPolicy::per_memory_region(nz!(1)))
            .hardware(hardware.clone())
            .thread_init({
                let initialized = Arc::clone(&initialized);
                move || {
                    assert_eq!(affinity(&hardware), BTreeSet::from([3]));
                    initialized.fetch_add(1, Ordering::Relaxed);
                }
            });
        assert_eq!(initialized.load(Ordering::Relaxed), 0);
        let pool = builder.build();
        assert_eq!(initialized.load(Ordering::Relaxed), 1);
        let worker_count = pool.inner.worker_handles.lock().unwrap().len();
        assert_eq!(worker_count, 1);
        assert_eq!(pool.inner.registry.initialized().count(), 1);
        assert_eq!(
            pool.inner
                .registry
                .get_or_init(GroupId::MemoryRegion(5))
                .startup
                .get(),
            Some(&Ok(()))
        );
    });
}

/// A valid prefix cannot start workers before a foreign entry fails complete-set validation.
#[test]
fn eager_selection_rejects_foreign_hardware_before_initialization() {
    let foreign = hardware();
    let hardware = hardware();
    let processors = hardware.all_processors();
    let mut selection = processors.processors().clone();
    // The foreign entry repeats a valid ID, so deduplication must not hide its provenance.
    selection.push(foreign.all_processors().processors().first().clone());
    let processors = processors.to_builder().take_exact(selection);
    // Rejection must precede startup; this counter observes calls rather than publishing readiness.
    let initialized = Arc::new(AtomicUsize::new(0));
    let builder = Pool::builder()
        .hardware(hardware)
        .eager(processors)
        .thread_init({
            let initialized = Arc::clone(&initialized);
            move || {
                initialized.fetch_add(1, Ordering::Relaxed);
            }
        });
    assert_panics(|| builder.build());
    assert_eq!(initialized.load(Ordering::Relaxed), 0);
}

/// Valid processor handles cannot vouch for the set's independent pinning backend.
#[test]
fn eager_selection_rejects_foreign_backing_hardware_before_initialization() {
    with_watchdog(|| {
        let foreign = hardware();
        let hardware = hardware();
        let processors = foreign
            .all_processors()
            .to_builder()
            .take_exact(hardware.all_processors().processors().clone());
        // No callback may start on rejected input; this counter does not publish readiness.
        let initialized = Arc::new(AtomicUsize::new(0));

        for kind in [
            PolicyKind::Processor,
            PolicyKind::MemoryRegion,
            PolicyKind::System,
        ] {
            let builder = Pool::builder()
                .hardware(hardware.clone())
                .worker_policy(policy(kind, nz!(1)))
                .eager(processors.clone())
                .thread_init({
                    let initialized = Arc::clone(&initialized);
                    move || {
                        initialized.fetch_add(1, Ordering::Relaxed);
                    }
                });
            assert_panics(|| builder.build());
            assert_eq!(initialized.load(Ordering::Relaxed), 0);
        }
    });
}

/// An unused eager pool needs neither an initializer nor a task to shut down all its workers.
#[test]
fn eager_unused_pool_without_initializer_joins_workers() {
    with_watchdog(|| {
        let hardware = hardware();
        // Idle siblings exercise cleanup independently of task execution and initializer callbacks.
        let pool = Pool::builder()
            .hardware(hardware.clone())
            .worker_policy(WorkerPolicy::per_system(nz!(2)))
            .eager(hardware.all_processors())
            .build();
        let inner = Arc::clone(&pool.inner);
        let scheduler = pool.scheduler();
        let worker_count = inner.worker_handles.lock().unwrap().len();
        assert_eq!(worker_count, 2);
        let state = inner.registry.get_or_init(GroupId::System);
        assert_eq!(state.get_tasks_spawned(), 0);
        assert!(state.task_pool.lock().unwrap().is_empty());

        drop(pool);

        assert!(inner.worker_handles.lock().unwrap().is_empty());
        assert!(state.shutdown_flag.load(Ordering::Acquire));
        assert!(!inner.poisoned.load(Ordering::Acquire));
        assert_retained_scheduler(&scheduler);
    });
}

/// Construction cannot serialize callbacks to completion, even across destination boundaries.
#[test]
fn eager_initializers_overlap_across_groups() {
    with_watchdog(|| {
        let hardware = hardware();
        let processors = hardware
            .all_processors()
            .filter(|processor| processor.id() != 1)
            .unwrap();
        let rendezvous = Arc::new(Barrier::new(processors.len()));
        let initialized = Arc::new(AtomicUsize::new(0));
        let pool = Pool::builder()
            .hardware(hardware)
            .worker_policy(WorkerPolicy::per_memory_region(nz!(1)))
            .eager(processors)
            .thread_init({
                let initialized = Arc::clone(&initialized);
                move || {
                    rendezvous.wait();
                    // The construction completion handshake, not this counter, publishes readiness.
                    initialized.fetch_add(1, Ordering::Relaxed);
                }
            })
            .build();
        assert_eq!(initialized.load(Ordering::Relaxed), 2);
        let worker_count = pool.inner.worker_handles.lock().unwrap().len();
        assert_eq!(worker_count, 2);
    });
}

/// Callback submission must not deadlock with eager construction's registration lock.
#[test]
fn eager_completion_wait_releases_registration_lock() {
    with_watchdog(|| {
        let scheduler = Arc::new(OnceLock::<Scheduler>::new());
        let hardware = hardware();
        let pool = Pool::builder()
            .hardware(hardware.clone())
            .worker_policy(WorkerPolicy::per_system(nz!(1)))
            .thread_init({
                let scheduler = Arc::downgrade(&scheduler);
                move || {
                    scheduler
                        .upgrade()
                        .unwrap()
                        .get()
                        .unwrap()
                        .spawn_and_forget(|| {});
                }
            })
            .build();
        scheduler.set(pool.scheduler()).unwrap();
        // Private entry permits publishing the scheduler first, exercising reentrancy that must
        // remain safe even though a public eager builder does not yet expose its pool.
        pool.inner.start_eager(hardware.all_processors());
        assert_eq!(
            pool.inner
                .registry
                .get_or_init(GroupId::System)
                .get_tasks_spawned(),
            1
        );
    });
}

/// Initial snapshots are already applied; an idle eager worker need not re-pin in its loop.
#[test]
fn eager_idle_workers_apply_the_initial_mask_once() {
    with_watchdog(|| {
        let hardware = hardware();
        let pool = Pool::builder()
            .hardware(hardware.clone())
            .worker_policy(WorkerPolicy::per_system(nz!(1)))
            .build();
        // Pool destruction joins every worker before the relaxed observation of pin calls.
        let pins = Arc::new(AtomicUsize::new(0));
        pool.inner
            .start_eager_with(hardware.all_processors(), thread::Builder::spawn, {
                let pins = Arc::clone(&pins);
                move |processors| {
                    processors.pin_current_thread_to();
                    pins.fetch_add(1, Ordering::Relaxed);
                }
            });
        block_on(pool.scheduler().spawn(|| {})).unwrap();
        drop(pool);
        assert_eq!(pins.load(Ordering::Relaxed), 1);
    });
}

/// Keeps each policy's concurrency explicit without coupling fixtures to the default count.
fn policy(kind: PolicyKind, count: NonZero<u32>) -> WorkerPolicy {
    match kind {
        PolicyKind::Processor => WorkerPolicy::per_processor(count),
        PolicyKind::MemoryRegion => WorkerPolicy::per_memory_region(count),
        PolicyKind::System => WorkerPolicy::per_system(count),
    }
}
