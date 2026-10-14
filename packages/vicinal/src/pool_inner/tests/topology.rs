//! Exact affinity, group sharing and wake notification coverage.

use std::collections::BTreeSet;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Barrier, mpsc};
use std::thread;

use futures::FutureExt;
use futures::executor::block_on;
use many_cpus::fake::{HardwareBuilder, ProcessorBuilder};
use many_cpus::{Processor, SystemHardware};
use new_zealand::nz;
use testing::with_watchdog;

use super::helpers::{NoopTask, affinity, hardware, pin, probe, regional_hardware};
use crate::pool_inner::refresh_affinity;
use crate::{GroupId, Pool, SubmissionTarget, WorkerPolicy, init_task};

/// Sparse IDs need one membership entry per eligible CPU, without registering absent submitters.
#[test]
fn sparse_processor_membership_is_compact_and_expands_by_identity() {
    with_watchdog(|| {
        // Small gaps expose ID-as-index mistakes without allocating a large fake topology.
        let hardware = SystemHardware::fake(
            HardwareBuilder::new()
                .processor(ProcessorBuilder::new().id(19).memory_region(2))
                .processor(ProcessorBuilder::new().id(3).memory_region(5))
                .processor(ProcessorBuilder::new().id(7).memory_region(2))
                .processor(ProcessorBuilder::new().id(11).memory_region(2)),
        );
        let pool = Pool::builder()
            .hardware(hardware.clone())
            .worker_policy(WorkerPolicy::per_memory_region(nz!(1)))
            .build();

        let membership = || {
            pool.inner
                .submitted_processors
                .iter()
                .map(|(id, submitted)| (*id, submitted.load(Ordering::Acquire)))
                .collect::<Vec<_>>()
        };

        assert_eq!(
            membership(),
            [(3, false), (7, false), (11, false), (19, false)]
        );
        assert_eq!(pool.inner.registry.initialized().count(), 0);
        let initial_worker_count = pool.inner.worker_handles.lock().unwrap().len();
        assert_eq!(initial_worker_count, 0);

        let scheduler = pool.scheduler();
        // Repeating a known submitter must leave the mask and revision unchanged.
        for (processor, expected) in [
            (19, BTreeSet::from([19])),
            (7, BTreeSet::from([7, 19])),
            (7, BTreeSet::from([7, 19])),
            (3, BTreeSet::from([3])),
        ] {
            pin(&hardware, processor);
            assert_eq!(
                block_on(scheduler.spawn({
                    let hardware = hardware.clone();
                    move || affinity(&hardware)
                }))
                .unwrap(),
                expected
            );
        }

        assert_eq!(
            membership(),
            [(3, true), (7, true), (11, false), (19, true)]
        );
        let region = pool.inner.registry.get_or_init(GroupId::MemoryRegion(2));
        assert_eq!(region.affinity_revision.load(Ordering::Acquire), 2);
        let worker_count = pool.inner.worker_handles.lock().unwrap().len();
        assert_eq!(worker_count, 2);
    });
}

/// Processor-local pools do not need the shared worker submission-membership table.
#[test]
fn processor_policy_does_not_allocate_submission_membership() {
    let pool = Pool::builder()
        .hardware(hardware())
        .worker_policy(WorkerPolicy::per_processor(nz!(1)))
        .build();
    assert!(pool.inner.submitted_processors.is_empty());
}

/// All submission APIs share a fixed count across regions without admitting unused processors.
#[test]
fn system_policy_shares_workers_and_accumulates_only_its_submitters() {
    // A minimal multiple-worker group exposes capacity multiplication during membership expansion.
    const WORKERS: u32 = 2;

    with_watchdog(|| {
        let hardware = regional_hardware();
        let pool = Pool::builder()
            .hardware(hardware.clone())
            .worker_policy(WorkerPolicy::per_system(nz!(WORKERS)))
            .build();

        assert_eq!(pool.inner.registry.initialized().count(), 0);
        assert!(pool.inner.worker_handles.lock().unwrap().is_empty());

        let scheduler = pool.scheduler();
        let mut expected = BTreeSet::new();

        // Cross regions and skip eligible CPUs while each API admits a new processor.
        for (variant, processor) in [0, 8, 3, 7].into_iter().enumerate() {
            pin(&hardware, processor);
            expected.insert(processor);
            let (sent, received) = mpsc::channel();
            let task = {
                let hardware = hardware.clone();
                move || sent.send(affinity(&hardware)).unwrap()
            };

            match variant {
                0 => block_on(scheduler.spawn(task)).unwrap(),
                1 => block_on(scheduler.spawn_urgent(task)).unwrap(),
                2 => scheduler.spawn_and_forget(task),
                3 => scheduler.spawn_urgent_and_forget(task),
                _ => unreachable!(),
            }

            assert_eq!(received.recv().unwrap(), expected);
            assert_eq!(pool.inner.registry.initialized().count(), 1);
            assert_eq!(
                pool.inner.worker_handles.lock().unwrap().len(),
                WORKERS as usize
            );
        }

        let state = pool.inner.registry.get_or_init(GroupId::System);
        assert_eq!(state.affinity_revision.load(Ordering::Acquire), 4);
        assert_eq!(state.get_tasks_spawned(), 4);
        assert_eq!(
            pool.inner
                .submitted_processors
                .iter()
                .map(|(id, submitted)| (*id, submitted.load(Ordering::Acquire)))
                .collect::<Vec<_>>(),
            [
                (0, true),
                (1, false),
                (2, false),
                (3, true),
                (7, true),
                (8, true)
            ]
        );

        // A separate pool's first submitter must not inherit or widen this pool's mask.
        pin(&hardware, 2);
        let independent = Pool::builder()
            .hardware(hardware.clone())
            .worker_policy(WorkerPolicy::per_system(nz!(1)))
            .build();
        assert_eq!(
            block_on(independent.scheduler().spawn({
                let hardware = hardware.clone();
                move || affinity(&hardware)
            }))
            .unwrap(),
            BTreeSet::from([2])
        );

        pin(&hardware, 0);
        assert_eq!(
            block_on(scheduler.spawn(move || affinity(&hardware))).unwrap(),
            expected
        );
        assert_eq!(state.affinity_revision.load(Ordering::Acquire), 4);
    });
}

/// Checks singleton worker affinity against quota-independent eligible topology.
#[test]
fn workers_pin_to_spawning_processors_despite_resource_quota() {
    with_watchdog(|| {
        let hardware = hardware();
        let processors = hardware.all_processors();
        assert!(hardware.processors().len() < processors.len());
        let pool = Pool::builder()
            .hardware(hardware.clone())
            .worker_policy(WorkerPolicy::per_processor(nz!(1)))
            .build();
        let scheduler = pool.scheduler();

        let handles = processors.spawn_threads(move |processor| {
            let handle = scheduler.spawn({
                let hardware = hardware.clone();
                move || affinity(&hardware)
            });
            assert_eq!(block_on(handle).unwrap(), BTreeSet::from([processor.id()]));
        });

        for handle in handles {
            handle.join().unwrap();
        }

        let worker_count = pool.inner.worker_handles.lock().unwrap().len();
        assert_eq!(worker_count, processors.len());
    });
}

/// Every admission path must publish membership before its task observes the worker mask.
#[test]
fn every_submission_variant_grows_the_region_mask() {
    with_watchdog(|| {
        let hardware = regional_hardware();
        let pool = Pool::builder()
            .hardware(hardware.clone())
            .worker_policy(WorkerPolicy::per_memory_region(nz!(1)))
            .build();
        let scheduler = pool.scheduler();
        let mut expected = BTreeSet::new();

        // Each variant is the first submission from a different CPU, leaving another CPU unused.
        for processor in 0..4 {
            pin(&hardware, processor);
            expected.insert(processor);
            let (sent, received) = mpsc::channel();
            let task = {
                let hardware = hardware.clone();
                move || sent.send(affinity(&hardware)).unwrap()
            };

            match processor {
                0 => block_on(scheduler.spawn(task)).unwrap(),
                1 => block_on(scheduler.spawn_urgent(task)).unwrap(),
                2 => scheduler.spawn_and_forget(task),
                3 => scheduler.spawn_urgent_and_forget(task),
                _ => unreachable!(),
            }

            assert_eq!(received.recv().unwrap(), expected);
            let worker_count = pool.inner.worker_handles.lock().unwrap().len();
            assert_eq!(worker_count, 1);
        }

        let state = pool.inner.registry.get_or_init(GroupId::MemoryRegion(2));
        assert_eq!(state.get_tasks_spawned(), 4);
    });
}

/// A submitter registered elsewhere must not widen this pool's or another region's mask.
#[test]
fn membership_is_isolated_by_pool_and_region() {
    with_watchdog(|| {
        let hardware = regional_hardware();
        pin(&hardware, 0);
        let pool = Pool::builder()
            .hardware(hardware.clone())
            .worker_policy(WorkerPolicy::per_memory_region(nz!(1)))
            .build();
        let scheduler = pool.scheduler();
        block_on(scheduler.spawn(|| {})).unwrap();

        pin(&hardware, 3);
        let independent = Pool::builder()
            .hardware(hardware.clone())
            .worker_policy(WorkerPolicy::per_memory_region(nz!(1)))
            .build();
        assert_eq!(
            block_on(independent.scheduler().spawn({
                let hardware = hardware.clone();
                move || affinity(&hardware)
            }))
            .unwrap(),
            BTreeSet::from([3])
        );

        pin(&hardware, 8);
        assert_eq!(
            block_on(scheduler.spawn(move || affinity(&hardware))).unwrap(),
            BTreeSet::from([8])
        );
        let worker_count = pool.inner.worker_handles.lock().unwrap().len();
        assert_eq!(worker_count, 2);
    });
}

/// Blocking all workers lets concurrent submitters widen one group before execution resumes.
#[test]
fn concurrent_additions_preserve_busy_workers_and_fixed_count() {
    assert_concurrent_additions(WorkerPolicy::per_memory_region(nz!(1)), [1, 2]);
}

/// Cross-region additions must preserve the same fixed capacity and task-boundary refresh.
#[test]
fn system_concurrent_additions_preserve_busy_workers_and_fixed_count() {
    assert_concurrent_additions(WorkerPolicy::per_system(nz!(1)), [1, 8]);
}

/// Exercises shared worker expansion without depending on which submitting thread runs first.
fn assert_concurrent_additions(policy: WorkerPolicy, processors: [u32; 2]) {
    with_watchdog(move || {
        let hardware = regional_hardware();
        pin(&hardware, 0);
        let destination = policy.destination(&hardware).group;
        let pool = Pool::builder()
            .hardware(hardware.clone())
            .worker_policy(policy)
            .build();
        let scheduler = pool.scheduler();
        let (entered, arrivals) = mpsc::channel();
        // Occupy the group so concurrent expansion cannot be confused with task execution.
        let (release, wait) = mpsc::channel();
        let running = scheduler.spawn({
            let hardware = hardware.clone();
            move || {
                entered.send(affinity(&hardware)).unwrap();
                wait.recv().unwrap();
                affinity(&hardware)
            }
        });
        assert_eq!(arrivals.recv().unwrap(), BTreeSet::from([0]));

        let start = Arc::new(Barrier::new(2));
        let submitters = processors.map(|processor| {
            thread::spawn({
                let start = Arc::clone(&start);
                let scheduler = scheduler.clone();
                let hardware = hardware.clone();
                move || {
                    pin(&hardware, processor);
                    start.wait();

                    scheduler.spawn(move || affinity(&hardware))
                }
            })
        });
        let pending = submitters.map(|submitter| submitter.join().unwrap());

        let state = pool.inner.registry.get_or_init(destination);
        assert_eq!(state.affinity_revision.load(Ordering::Acquire), 3);
        let worker_count = pool.inner.worker_handles.lock().unwrap().len();
        assert_eq!(worker_count, 1);

        release.send(()).unwrap();
        // No preemption or mid-task repinning: the prior mask remains a valid subset.
        assert_eq!(block_on(running).unwrap(), BTreeSet::from([0]));
        for task in pending {
            assert_eq!(
                block_on(task).unwrap(),
                BTreeSet::from([0, processors[0], processors[1]])
            );
        }
    });
}

/// Pin acknowledgments prove idle workers refresh without needing one task per worker.
#[test]
fn idle_workers_refresh_even_without_a_task_for_each_worker() {
    assert_idle_workers_refresh(WorkerPolicy::per_memory_region(nz!(2)), 1);
}

/// System workers must also refresh when an idle worker receives no task after expansion.
#[test]
fn system_idle_workers_refresh_even_without_a_task_for_each_worker() {
    assert_idle_workers_refresh(WorkerPolicy::per_system(nz!(2)), 8);
}

/// Expands affinity while each worker holds a fresh registered wait at the initial revision.
fn assert_idle_workers_refresh(policy: WorkerPolicy, next_processor: u32) {
    // Sibling workers expose whether expansion wakes workers without queued tasks.
    const WORKERS: u32 = 2;

    with_watchdog(move || {
        let hardware = regional_hardware();
        pin(&hardware, 0);
        let pool = Pool::builder()
            .hardware(hardware.clone())
            .worker_policy(policy)
            .build();

        // Validate capacity before waiting for an acknowledgment from every worker.
        assert_eq!(pool.inner.policy.count().get(), WORKERS);
        let target = pool.inner.policy.destination(&hardware);
        let state = pool.inner.registry.get_or_init(target.group);
        let slot = state.task_pool.lock().unwrap().alloc_uninit_box();

        // Finish initial admission before starting threads, leaving no earlier notifications
        // that could satisfy the registered waits whose expansion wakeup is under test.
        pool.inner
            .enqueue_with(target, state, init_task(slot, NoopTask), false, |_| {});
        let initial_revision = state.affinity_revision.load(Ordering::Acquire);

        let (waiting, waiters) = mpsc::channel();
        assert!(
            state
                .before_wait
                .set(Box::new(move |worker_index, revision| {
                    if revision == initial_revision {
                        let (resume, wait) = mpsc::channel();
                        waiting
                            .send((worker_index, thread::current().id(), resume))
                            .unwrap();
                        wait.recv().unwrap();
                    }
                }))
                .is_ok()
        );

        let (refreshed, updates) = mpsc::channel();
        pool.inner.start_workers(
            target,
            &mut pool.inner.worker_handles.lock().unwrap(),
            thread::Builder::spawn,
            move |set| {
                set.pin_current_thread_to();
                if set.len() > 1 {
                    refreshed.send(thread::current().id()).unwrap();
                }
            },
        );

        let (first_index, first_id, first_resume) = waiters.recv().unwrap();
        let (second_index, second_id, second_resume) = waiters.recv().unwrap();
        assert_eq!(
            BTreeSet::from([first_index, second_index]),
            BTreeSet::from([0, 1])
        );
        assert_ne!(first_id, second_id);

        pin(&hardware, next_processor);
        let task = pool.scheduler().spawn(|| {});
        first_resume.send(()).unwrap();
        second_resume.send(()).unwrap();
        block_on(task).unwrap();

        let first = updates.recv().unwrap();
        let second = updates.recv().unwrap();
        assert_ne!(first, second);
        assert!([first_id, second_id].contains(&first));
        assert!([first_id, second_id].contains(&second));
        let worker_count = pool.inner.worker_handles.lock().unwrap().len();
        assert_eq!(worker_count, WORKERS as usize);
    });
}

/// A registered listener detects unnecessary broadcasts independently of task consumption.
#[test]
fn known_submitter_does_not_broadcast_or_rebuild_affinity() {
    let hardware = regional_hardware();
    let pool = Pool::builder()
        .hardware(hardware)
        .worker_policy(WorkerPolicy::per_memory_region(nz!(1)))
        .build();
    let target = SubmissionTarget {
        group: GroupId::MemoryRegion(2),
        processor: 0,
    };
    let state = pool.inner.registry.get_or_init(target.group);

    // Drive admission with no threads so listener notifications are directly observable.
    state.startup.set(Ok(())).unwrap();
    let allocate = || {
        let slot = state.task_pool.lock().unwrap().alloc_uninit_box();
        init_task(slot, NoopTask)
    };

    let first = state.wake_event.listen();
    let second = state.wake_event.listen();
    pool.inner.enqueue(target, state, allocate(), false);
    assert_eq!(first.now_or_never(), Some(()));
    assert_eq!(second.now_or_never(), Some(()));

    let revision = state.affinity_revision.load(Ordering::Acquire);
    let snapshot = Arc::clone(state.affinity.lock().unwrap().as_ref().unwrap());
    let first = state.wake_event.listen();
    let second = state.wake_event.listen();

    pool.inner.enqueue(target, state, allocate(), false);
    assert_eq!(first.now_or_never(), Some(()));
    assert_eq!(second.now_or_never(), None);
    assert_eq!(state.affinity_revision.load(Ordering::Acquire), revision);
    assert!(Arc::ptr_eq(
        &snapshot,
        state.affinity.lock().unwrap().as_ref().unwrap()
    ));

    let mut applied = 0;
    refresh_affinity(state, &mut applied, |set| {
        assert_eq!(
            set.iter().map(Processor::id).collect::<BTreeSet<_>>(),
            BTreeSet::from([0])
        );
        drop(state.affinity.try_lock().unwrap());
    });
    refresh_affinity(state, &mut applied, |_| {
        panic!("known revision must not repin")
    });
}

/// Separate listeners represent workers that must each wake for separately blocking work.
#[test]
fn separate_enqueues_notify_separate_waiters() {
    let pool = Pool::builder().hardware(hardware()).build();
    let destination = GroupId::Processor(0);
    let state = pool.inner.registry.get_or_init(destination);
    let first = state.wake_event.listen();
    let second = state.wake_event.listen();

    // No OS workers are needed to verify the notification protocol.
    state.startup.set(Ok(())).unwrap();
    let target = SubmissionTarget {
        group: destination,
        processor: 0,
    };

    pool.inner
        .enqueue(target, state, probe(state, || {}), false);
    pool.inner.enqueue(target, state, probe(state, || {}), true);
    assert_eq!(first.now_or_never(), Some(()));
    assert_eq!(second.now_or_never(), Some(()));
}
