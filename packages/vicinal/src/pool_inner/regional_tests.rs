//! Exact-mask and admission regressions for cumulative regional affinity.

use std::collections::BTreeSet;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, mpsc};
use std::thread;

use futures::FutureExt;
use futures::executor::block_on;
use many_cpus::fake::{HardwareBuilder, ProcessorBuilder};
use many_cpus::{Processor, SystemHardware};
use new_zealand::nz;
use testing::{assert_panics, with_watchdog};

use crate::pool_inner::refresh_affinity;
use crate::{GroupId, Pool, SubmissionTarget, VicinalTask, WorkerPolicy, init_task};

/// A larger region than the submitter set, plus a distinct sparse region.
fn hardware() -> SystemHardware {
    let mut builder = HardwareBuilder::new().max_processor_time(1.0);
    for id in [0, 1, 2, 3, 7] {
        builder = builder.processor(ProcessorBuilder::new().id(id).memory_region(2));
    }
    builder = builder.processor(ProcessorBuilder::new().id(8).memory_region(5));
    SystemHardware::fake(builder)
}

/// Pins submitters through the same fake PAL used to inspect workers' configured masks.
fn pin(hardware: &SystemHardware, processor: u32) {
    hardware
        .all_processors()
        .to_builder()
        .filter(|p| p.id() == processor)
        .take_all()
        .unwrap()
        .pin_current_thread_to();
}

/// `thread_processors()` is region-granular; query the PAL to see restricted multi-CPU masks.
fn mask(hardware: &SystemHardware) -> BTreeSet<u32> {
    hardware
        .all_processors()
        .to_builder()
        .where_available_for_current_thread()
        .take_all()
        .unwrap()
        .iter()
        .map(Processor::id)
        .collect()
}

#[test]
fn every_submission_variant_grows_the_region_mask() {
    with_watchdog(|| {
        let hardware = hardware();
        let pool = Pool::builder()
            .hardware(hardware.clone())
            .worker_policy(WorkerPolicy::fixed_per_memory_region(nz!(1)))
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
                move || sent.send(mask(&hardware)).unwrap()
            };
            match processor {
                0 => block_on(scheduler.spawn(task)),
                1 => block_on(scheduler.spawn_urgent(task)),
                2 => scheduler.spawn_and_forget(task),
                3 => scheduler.spawn_urgent_and_forget(task),
                _ => unreachable!(),
            }
            assert_eq!(received.recv().unwrap(), expected);
            assert_eq!(pool.inner.worker_handles.lock().unwrap().len(), 1);
        }
        let state = pool.inner.registry.get_or_init(GroupId::MemoryRegion(2));
        assert_eq!(state.get_tasks_spawned(), 4);
    });
}

#[test]
fn membership_is_isolated_by_pool_and_region() {
    with_watchdog(|| {
        let hardware = hardware();
        pin(&hardware, 0);
        let pool = Pool::builder()
            .hardware(hardware.clone())
            .worker_policy(WorkerPolicy::fixed_per_memory_region(nz!(1)))
            .build();
        let scheduler = pool.scheduler();
        block_on(scheduler.spawn(|| {}));
        pin(&hardware, 3);
        let independent = Pool::builder()
            .hardware(hardware.clone())
            .worker_policy(WorkerPolicy::fixed_per_memory_region(nz!(1)))
            .build();
        assert_eq!(
            block_on(independent.scheduler().spawn({
                let hardware = hardware.clone();
                move || mask(&hardware)
            })),
            BTreeSet::from([3])
        );
        pin(&hardware, 8);
        assert_eq!(
            block_on(scheduler.spawn(move || mask(&hardware))),
            BTreeSet::from([8])
        );
        assert_eq!(pool.inner.worker_handles.lock().unwrap().len(), 2);
    });
}

#[test]
fn concurrent_additions_preserve_busy_workers_and_fixed_count() {
    with_watchdog(|| {
        let hardware = hardware();
        pin(&hardware, 0);
        let pool = Pool::builder()
            .hardware(hardware.clone())
            .worker_policy(WorkerPolicy::fixed_per_memory_region(nz!(1)))
            .build();
        let scheduler = pool.scheduler();
        let (entered, arrivals) = mpsc::channel();
        // Occupy the group so concurrent expansion cannot be confused with task execution.
        let (release, wait) = mpsc::channel();
        let running = scheduler.spawn({
            let hardware = hardware.clone();
            move || {
                entered.send(mask(&hardware)).unwrap();
                wait.recv().unwrap();
                mask(&hardware)
            }
        });
        assert_eq!(arrivals.recv().unwrap(), BTreeSet::from([0]));
        let start = Arc::new(Barrier::new(2));
        let submitters = [1, 2].map(|processor| {
            thread::spawn({
                let start = Arc::clone(&start);
                let scheduler = scheduler.clone();
                let hardware = hardware.clone();
                move || {
                    pin(&hardware, processor);
                    start.wait();
                    scheduler.spawn(move || mask(&hardware))
                }
            })
        });
        let pending = submitters.map(|submitter| submitter.join().unwrap());
        let state = pool.inner.registry.get_or_init(GroupId::MemoryRegion(2));
        assert_eq!(state.affinity_revision.load(Ordering::Acquire), 3);
        assert_eq!(pool.inner.worker_handles.lock().unwrap().len(), 1);
        release.send(()).unwrap();
        // No preemption or mid-task repinning: the prior mask remains a valid subset.
        assert_eq!(block_on(running), BTreeSet::from([0]));
        for task in pending {
            assert_eq!(block_on(task), BTreeSet::from([0, 1, 2]));
        }
    });
}

/// Allows an injected thread adapter to initialize a group with ordinary executable work.
struct NoopTask;

impl VicinalTask for NoopTask {
    fn call(self: Pin<&mut Self>) {}
}

#[test]
fn idle_workers_refresh_even_without_a_task_for_each_worker() {
    with_watchdog(|| {
        let hardware = hardware();
        pin(&hardware, 0);
        let pool = Pool::builder()
            .hardware(hardware.clone())
            .worker_policy(WorkerPolicy::fixed_per_memory_region(nz!(2)))
            .build();
        let target = pool.inner.policy.destination(&hardware);
        let state = pool.inner.registry.get_or_init(target.group);
        let slot = state.task_pool.lock().unwrap().alloc_uninit_box();
        let (refreshed, updates) = mpsc::channel();
        pool.inner
            .enqueue_with(target, state, init_task(slot, NoopTask), false, |handles| {
                pool.inner
                    .start_workers(target, handles, thread::Builder::spawn, move |set| {
                        set.pin_current_thread_to();
                        if set.len() > 1 {
                            refreshed.send(thread::current().id()).unwrap();
                        }
                    });
            });
        block_on(pool.scheduler().spawn(|| {}));
        pin(&hardware, 1);
        block_on(pool.scheduler().spawn(|| {}));
        let first = updates.recv().unwrap();
        let second = updates.recv().unwrap();
        assert_ne!(first, second);
        assert_eq!(pool.inner.worker_handles.lock().unwrap().len(), 2);
    });
}

#[test]
fn known_submitter_does_not_broadcast_or_rebuild_affinity() {
    let hardware = hardware();
    let pool = Pool::builder()
        .hardware(hardware)
        .worker_policy(WorkerPolicy::fixed_per_memory_region(nz!(1)))
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

#[test]
fn post_shutdown_submissions_do_not_expand_or_activate() {
    let hardware = hardware();
    pin(&hardware, 0);
    let pool = Pool::builder()
        .hardware(hardware.clone())
        .worker_policy(WorkerPolicy::fixed_per_memory_region(nz!(1)))
        .build();
    let scheduler = pool.scheduler();
    block_on(scheduler.spawn(|| {}));
    let inner = Arc::clone(&pool.inner);
    drop(pool);
    for processor in [1, 8] {
        pin(&hardware, processor);
        let task = scheduler.spawn(|| panic!("abandoned work must not execute"));
        assert_panics(|| block_on(task));
    }
    let existing = inner.registry.get_or_init(GroupId::MemoryRegion(2));
    assert_eq!(existing.affinity_revision.load(Ordering::Acquire), 1);
    let unused = inner.registry.get_or_init(GroupId::MemoryRegion(5));
    assert!(unused.startup.get().is_none());
    assert_eq!(unused.affinity_revision.load(Ordering::Acquire), 0);
    assert!(inner.worker_handles.lock().unwrap().is_empty());
}

/// Tracks cleanup of queued work after a worker fails its affinity refresh.
struct OnDrop {
    dropped: Arc<AtomicUsize>,
}

impl Drop for OnDrop {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::Release);
    }
}

#[test]
fn refresh_failure_stops_pool_and_disconnects_accepted_work() {
    with_watchdog(|| {
        let hardware = hardware();
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
            let capture = OnDrop {
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
