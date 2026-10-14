//! Deterministic shutdown and failure races through the production worker loop.

use std::cell::Cell;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use many_cpus::ProcessorSet;
use new_zealand::nz;
use testing::{assert_panics, with_watchdog};

use super::helpers::{hardware, pin, probe};
use crate::pool_inner::worker_loop;
use crate::{GroupId, Pool, WorkerGroup, WorkerPolicy};

/// The pool-wide stop must abandon dequeued work even before its group receives a stop signal.
#[test]
fn shutdown_before_dequeue_abandons_without_another_refresh() {
    with_watchdog(|| {
        let (pool, destination) = regional_pool();
        let state = pool.inner.registry.get_or_init(destination);
        let dropped = Arc::new(AtomicBool::new(false));
        state.regular_queue.lock().unwrap().push_back(probe(state, {
            let inner = Arc::clone(&pool.inner);
            let dropped = Arc::clone(&dropped);
            move || {
                let state = inner.registry.get_or_init(destination);
                assert!(inner.shutdown.load(Ordering::Acquire));
                assert!(!inner.poisoned.load(Ordering::Acquire));
                assert!(!state.shutdown_flag.load(Ordering::Acquire));
                drop(state.regular_queue.try_lock().unwrap());
                drop(state.affinity.try_lock().unwrap());
                dropped.store(true, Ordering::Release);
            }
        }));

        let pins = Cell::new(0);
        worker_loop(&pool.inner, destination, 0, 0, |_| {
            pins.set(pins.get() + 1);
            let queued = state.regular_queue.lock().unwrap().len();
            assert_eq!(queued, 1);
            // The outer stop check has passed, but this iteration has not dequeued its task.
            // A pending revision makes an unnecessary post-stop refresh observable.
            publish_expansion(&pool, destination);
            pool.inner.shutdown.store(true, Ordering::Release);
        });

        assert_eq!(pins.get(), 1);
        assert!(dropped.load(Ordering::Acquire));
        assert!(state.task_pool.lock().unwrap().is_empty());
    });
}

/// Shutdown during pinning must be rechecked before the dequeued task can start.
#[test]
fn shutdown_during_dequeued_refresh_abandons_without_poisoning() {
    with_watchdog(|| {
        let (pool, destination) = regional_pool();
        let state = pool.inner.registry.get_or_init(destination);
        let dropped = Arc::new(AtomicBool::new(false));
        state.urgent_queue.lock().unwrap().push_back(probe(state, {
            let inner = Arc::clone(&pool.inner);
            let dropped = Arc::clone(&dropped);
            move || {
                let state = inner.registry.get_or_init(destination);
                assert!(inner.shutdown.load(Ordering::Acquire));
                assert!(!inner.poisoned.load(Ordering::Acquire));
                assert!(!state.shutdown_flag.load(Ordering::Acquire));
                drop(state.urgent_queue.try_lock().unwrap());
                drop(state.affinity.try_lock().unwrap());
                dropped.store(true, Ordering::Release);
            }
        }));

        let pins = Cell::new(0);
        worker_loop(&pool.inner, destination, 0, 0, |set| {
            pins.set(pins.get() + 1);
            match pins.get() {
                1 => {
                    assert_eq!(set.len(), 1);
                    let queued = state.urgent_queue.lock().unwrap().len();
                    assert_eq!(queued, 1);
                    publish_expansion(&pool, destination);
                }
                2 => {
                    assert_eq!(set.len(), 2);
                    assert!(state.urgent_queue.lock().unwrap().is_empty());
                    // Leave the group signal untouched: only the post-refresh global check
                    // can prevent this already-dequeued task from executing.
                    pool.inner.shutdown.store(true, Ordering::Release);
                }
                _ => panic!("the worker must exit after abandoning the dequeued task"),
            }
        });

        assert_eq!(pins.get(), 2);
        assert!(dropped.load(Ordering::Acquire));
        assert!(!pool.inner.poisoned.load(Ordering::Acquire));
        assert!(state.task_pool.lock().unwrap().is_empty());
    });
}

/// Dequeued-task destruction must see the worker loop's real pool-wide rejection callback.
#[test]
fn dequeued_refresh_failure_rejects_reentrant_submissions_before_cleanup() {
    struct AffinityFailure;

    with_watchdog(|| {
        let (pool, destination) = regional_pool();
        let state = pool.inner.registry.get_or_init(destination);
        let rejected = Arc::new(AtomicBool::new(false));
        state.regular_queue.lock().unwrap().push_back(probe(state, {
            let scheduler = pool.scheduler();
            let inner = Arc::clone(&pool.inner);
            let rejected = Arc::clone(&rejected);
            move || {
                let state = inner.registry.get_or_init(destination);
                assert!(inner.poisoned.load(Ordering::Acquire));
                assert!(inner.shutdown.load(Ordering::Acquire));
                drop(state.regular_queue.try_lock().unwrap());
                drop(state.affinity.try_lock().unwrap());
                drop(inner.worker_handles.try_lock().unwrap());
                assert_panics(|| scheduler.spawn(|| {}));
                assert_panics(|| scheduler.spawn_urgent(|| {}));
                assert_panics(|| scheduler.spawn_and_forget(|| {}));
                assert_panics(|| scheduler.spawn_urgent_and_forget(|| {}));
                // Cleanup contains secondary panics, so only a completed probe proves success.
                rejected.store(true, Ordering::Release);
            }
        }));

        let pins = Cell::new(0);
        let payload = panic::catch_unwind(AssertUnwindSafe(|| {
            worker_loop(&pool.inner, destination, 0, 0, |set| {
                pins.set(pins.get() + 1);
                match pins.get() {
                    1 => {
                        assert_eq!(set.len(), 1);
                        let queued = state.regular_queue.lock().unwrap().len();
                        assert_eq!(queued, 1);
                        publish_expansion(&pool, destination);
                    }
                    2 => {
                        assert_eq!(set.len(), 2);
                        assert!(state.regular_queue.lock().unwrap().is_empty());
                        panic::panic_any(AffinityFailure);
                    }
                    _ => panic!("a failed refresh must terminate the worker"),
                }
            });
        }))
        .unwrap_err();

        assert!(payload.is::<AffinityFailure>());
        assert_eq!(pins.get(), 2);
        assert!(rejected.load(Ordering::Acquire));
        assert!(pool.inner.worker_handles.lock().unwrap().is_empty());
        assert!(state.task_pool.lock().unwrap().is_empty());
    });
}

/// Starts the loop with a singleton revision, without background workers racing the test.
fn regional_pool() -> (Pool, GroupId) {
    let hardware = hardware();
    pin(&hardware, 0);
    let pool = Pool::builder()
        .hardware(hardware.clone())
        .worker_policy(WorkerPolicy::per_memory_region(nz!(1)))
        .build();
    let target = pool.inner.policy.destination(&hardware);
    let state = pool.inner.registry.get_or_init(target.group);

    publish_affinity(
        state,
        hardware
            .all_processors()
            .to_builder()
            .filter(|processor| processor.id() == target.processor)
            .take_all()
            .unwrap(),
    );

    (pool, target.group)
}

/// Models admission while pinning a copied snapshot, forcing a refresh after dequeueing.
fn publish_expansion(pool: &Pool, destination: GroupId) {
    publish_affinity(
        pool.inner.registry.get_or_init(destination),
        pool.inner
            .hardware
            .all_processors()
            .to_builder()
            .filter(|processor| destination.contains(processor))
            .take_all()
            .unwrap(),
    );
}

/// Publishes snapshots without task admission so tests control the loop's exact queue contents.
fn publish_affinity(state: &WorkerGroup, set: ProcessorSet) {
    let mut affinity = state.affinity.lock().unwrap();
    *affinity = Some(Arc::new(set));
    state.affinity_revision.fetch_add(1, Ordering::Release);
}
