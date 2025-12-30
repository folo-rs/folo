//! Integration tests for the vicinal worker pool.
//!
//! These tests verify full pool functionality with real threads. They are ignored under Miri
//! because Miri does not support thread spawning and platform-specific calls.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use events_once::EventPool;
use futures::executor::block_on;
use testing::with_watchdog;
use vicinal::Pool;

#[cfg_attr(miri, ignore)]
#[test]
fn spawn_and_await_value() {
    with_watchdog(|| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();

        let handle = scheduler.spawn(|| 42);
        let result = block_on(handle);

        assert_eq!(result, 42);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn spawn_and_await_unit() {
    with_watchdog(|| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();

        let handle = scheduler.spawn(|| {});
        block_on(handle);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
#[should_panic]
fn spawn_and_await_panic() {
    with_watchdog(|| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();

        let handle = scheduler.spawn(|| panic!("intentional panic"));
        block_on(handle);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn clone_scheduler_spawn_from_both() {
    with_watchdog(|| {
        let pool = Pool::new();
        let scheduler1 = pool.scheduler();
        let scheduler2 = scheduler1.clone();

        let handle1 = scheduler1.spawn(|| 1);
        let handle2 = scheduler2.spawn(|| 2);

        assert_eq!(block_on(handle1), 1);
        assert_eq!(block_on(handle2), 2);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn nested_spawn_via_captured_scheduler() {
    with_watchdog(|| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();

        let scheduler_clone = scheduler.clone();
        let outer_handle = scheduler.spawn(move || scheduler_clone.spawn(|| 99));

        let inner_handle = block_on(outer_handle);
        let result = block_on(inner_handle);

        assert_eq!(result, 99);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn drop_pool_with_no_tasks() {
    with_watchdog(|| {
        let pool = Pool::new();
        drop(pool);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn drop_pool_after_awaiting_task() {
    with_watchdog(|| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();

        let handle = scheduler.spawn(|| 42);
        assert_eq!(block_on(handle), 42);

        drop(pool);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn spawn_many_tasks_await_all() {
    with_watchdog(|| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();

        for (i, handle) in (0..100).map(|i| scheduler.spawn(move || i * 2)).enumerate() {
            assert_eq!(block_on(handle), i * 2);
        }
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn scheduler_sent_to_another_thread() {
    with_watchdog(|| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();

        let handle = thread::spawn(move || {
            let task_handle = scheduler.spawn(|| 123);
            block_on(task_handle)
        });

        let result = handle.join().unwrap();
        assert_eq!(result, 123);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn concurrent_spawns_from_multiple_threads() {
    with_watchdog(|| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();
        let completed = Arc::new(AtomicUsize::new(0));

        let thread_handles: Vec<_> = std::iter::repeat_with(|| {
            let scheduler = scheduler.clone();
            let completed = Arc::clone(&completed);

            thread::spawn(move || {
                let handles: Vec<_> = (0..25)
                    .map(|i| {
                        let completed = Arc::clone(&completed);
                        scheduler.spawn(move || {
                            completed.fetch_add(1, Ordering::Relaxed);
                            i
                        })
                    })
                    .collect();

                for handle in handles {
                    block_on(handle);
                }
            })
        })
        .take(4)
        .collect();

        for handle in thread_handles {
            handle.join().unwrap();
        }

        assert_eq!(completed.load(Ordering::Relaxed), 100);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn spawn_and_forget_completes() {
    with_watchdog(|| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();
        let event_pool = EventPool::<()>::new();

        let (tx, rx) = event_pool.rent();
        scheduler.spawn_and_forget(move || {
            // Signal completion via the event.
            tx.send(());
        });

        // Wait for the task to complete.
        block_on(rx).unwrap();
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn spawn_urgent_and_forget_completes() {
    with_watchdog(|| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();
        let event_pool = EventPool::<()>::new();

        let (tx, rx) = event_pool.rent();
        scheduler.spawn_urgent_and_forget(move || {
            // Signal completion via the event.
            tx.send(());
        });

        // Wait for the task to complete.
        block_on(rx).unwrap();
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn spawn_and_forget_multiple_tasks() {
    with_watchdog(|| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();
        let event_pool = EventPool::<()>::new();
        let completed = Arc::new(AtomicUsize::new(0));

        let mut receivers = Vec::new();

        for _ in 0..50 {
            let (tx, rx) = event_pool.rent();
            let completed = Arc::clone(&completed);
            receivers.push(rx);

            scheduler.spawn_and_forget(move || {
                completed.fetch_add(1, Ordering::Relaxed);
                tx.send(());
            });
        }

        // Wait for all tasks to complete.
        for rx in receivers {
            block_on(rx).unwrap();
        }

        assert_eq!(completed.load(Ordering::Relaxed), 50);
    });
}
