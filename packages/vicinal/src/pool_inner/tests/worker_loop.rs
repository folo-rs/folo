//! Deterministic shutdown and failure races through the production worker loop.

use std::cell::Cell;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use many_cpus::ProcessorSet;
use testing::{assert_panics, with_watchdog};

use super::helpers::{TestPolicy, hardware, probe};
use crate::pool_inner::worker_loop_with;
use crate::{Pool, WorkerGroup, WorkerGroupState};

/// The pool-wide stop must abandon dequeued work even before its group receives a stop signal.
#[test]
fn shutdown_before_dequeue_abandons_without_another_refresh() {
    with_watchdog(|| {
        let (pool, group) = group_pool();
        let state = group.state();
        let dropped = Arc::new(AtomicBool::new(false));
        state.regular_queue.lock().unwrap().push_back(probe(state, {
            let inner = Arc::clone(&pool.inner);
            let dropped = Arc::clone(&dropped);
            let group = group.clone();
            move || {
                let state = group.state();
                assert!(inner.runtime.is_shutdown());
                assert!(!inner.runtime.is_poisoned());
                assert!(!state.shutdown_flag.load(Ordering::Acquire));
                drop(state.regular_queue.try_lock().unwrap());
                drop(state.affinity.try_read().unwrap());
                dropped.store(true, Ordering::Release);
            }
        }));

        let pins = Cell::new(0);
        worker_loop_with(&pool.inner.runtime, state, 0, 0, |_| {
            pins.set(pins.get() + 1);
            let queued = state.regular_queue.lock().unwrap().len();
            assert_eq!(queued, 1);
            // The outer stop check has passed, but this iteration has not dequeued its task.
            // A pending revision makes an unnecessary post-stop refresh observable.
            publish_expansion(&pool, &group);
            pool.inner.runtime.publish_shutdown_without_group_signals();
        });

        pool.inner.shutdown_and_wait_for_workers();
        assert_eq!(pins.get(), 1);
        assert!(dropped.load(Ordering::Acquire));
        assert!(state.task_pool.lock().unwrap().is_empty());
    });
}

/// Shutdown during pinning must be rechecked before the dequeued task can start.
#[test]
fn shutdown_during_dequeued_refresh_abandons_without_poisoning() {
    with_watchdog(|| {
        let (pool, group) = group_pool();
        let state = group.state();
        let dropped = Arc::new(AtomicBool::new(false));
        state.urgent_queue.lock().unwrap().push_back(probe(state, {
            let inner = Arc::clone(&pool.inner);
            let dropped = Arc::clone(&dropped);
            let group = group.clone();
            move || {
                let state = group.state();
                assert!(inner.runtime.is_shutdown());
                assert!(!inner.runtime.is_poisoned());
                assert!(state.shutdown_flag.load(Ordering::Acquire));
                drop(state.urgent_queue.try_lock().unwrap());
                drop(state.affinity.try_read().unwrap());
                dropped.store(true, Ordering::Release);
            }
        }));

        let pins = Cell::new(0);
        worker_loop_with(&pool.inner.runtime, state, 0, 0, |set| {
            pins.set(pins.get() + 1);
            match pins.get() {
                1 => {
                    assert_eq!(set.len(), 1);
                    let queued = state.urgent_queue.lock().unwrap().len();
                    assert_eq!(queued, 1);
                    publish_expansion(&pool, &group);
                }
                2 => {
                    assert_eq!(set.len(), 2);
                    assert!(state.urgent_queue.lock().unwrap().is_empty());
                    // The pool and group are both stopping, but this task has already been
                    // dequeued; the post-refresh global check must still prevent execution.
                    pool.inner.runtime.stop_workers();
                }
                _ => panic!("the worker must exit after abandoning the dequeued task"),
            }
        });

        assert_eq!(pins.get(), 2);
        assert!(dropped.load(Ordering::Acquire));
        assert!(!pool.inner.runtime.is_poisoned());
        assert!(state.task_pool.lock().unwrap().is_empty());
    });
}

/// Dequeued-task destruction must see the worker loop's real pool-wide rejection callback.
#[test]
fn dequeued_refresh_failure_rejects_reentrant_submissions_before_cleanup() {
    struct AffinityFailure;

    with_watchdog(|| {
        let (pool, group) = group_pool();
        let state = group.state();
        let rejected = Arc::new(AtomicBool::new(false));
        state.regular_queue.lock().unwrap().push_back(probe(state, {
            let scheduler = pool.scheduler();
            let inner = Arc::clone(&pool.inner);
            let rejected = Arc::clone(&rejected);
            let group = group.clone();
            move || {
                let state = group.state();
                assert!(inner.runtime.is_poisoned());
                assert!(inner.runtime.is_shutdown());
                drop(state.regular_queue.try_lock().unwrap());
                drop(state.affinity.try_read().unwrap());
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
            worker_loop_with(&pool.inner.runtime, state, 0, 0, |set| {
                pins.set(pins.get() + 1);
                match pins.get() {
                    1 => {
                        assert_eq!(set.len(), 1);
                        let queued = state.regular_queue.lock().unwrap().len();
                        assert_eq!(queued, 1);
                        publish_expansion(&pool, &group);
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
        assert_eq!(pool.inner.runtime.active_workers(), 0);
        assert!(state.task_pool.lock().unwrap().is_empty());
    });
}

/// Starts the loop with a singleton revision, without background workers racing the test.
fn group_pool() -> (Pool, WorkerGroup) {
    let hardware = hardware();
    let policy = TestPolicy::new(0);
    let pool = Pool::builder()
        .hardware(hardware.clone())
        .worker_policy(policy.clone())
        .build();
    let group = policy.group();
    let state = group.state();

    publish_affinity(
        state,
        hardware
            .all_processors()
            .to_builder()
            .filter(|processor| processor.id() == 0)
            .take_all()
            .unwrap(),
    );

    (pool, group)
}

/// Models admission while pinning a copied snapshot, forcing a refresh after dequeueing.
fn publish_expansion(pool: &Pool, group: &WorkerGroup) {
    publish_affinity(
        group.state(),
        pool.inner
            .hardware
            .all_processors()
            .to_builder()
            .filter(|processor| processor.id() < 2)
            .take_all()
            .unwrap(),
    );
}

/// Publishes snapshots without task admission so tests control the loop's exact queue contents.
fn publish_affinity(state: &WorkerGroupState, set: ProcessorSet) {
    let mut affinity = state.affinity.write().unwrap();
    *affinity = Some(Arc::new(set));
    state.affinity_revision.fetch_add(1, Ordering::Release);
}
