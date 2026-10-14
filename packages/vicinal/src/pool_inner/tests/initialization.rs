//! Initialization ordering, thread-local state and failure behavior on simulated hardware.

use std::cell::Cell;
use std::collections::{BTreeSet, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, Mutex, OnceLock, mpsc};
use std::{iter, thread};

use futures::executor::block_on;
use new_zealand::nz;
use testing::{assert_panics, with_watchdog};

use super::helpers::{affinity, hardware, pin};
use crate::{PolicyKind, Pool, Scheduler, WorkerPolicy};

thread_local! {
    static INITIALIZED: Cell<bool> = const { Cell::new(false) };
}

/// Processor-local workers initialize once, including idle siblings.
#[test]
fn processor_initialization_is_lazy_and_once_per_worker() {
    assert_lazy_initialization(PolicyKind::Processor);
}

/// Regional membership expansion reuses initialized workers rather than starting new ones.
#[test]
fn regional_initialization_is_lazy_and_once_per_worker() {
    assert_lazy_initialization(PolicyKind::MemoryRegion);
}

/// System membership expansion across regions retains each worker's initialized state.
#[test]
fn system_initialization_is_lazy_and_once_per_worker() {
    assert_lazy_initialization(PolicyKind::System);
}

/// Shares the callback contract assertions while keeping each policy within the Miri budget.
fn assert_lazy_initialization(kind: PolicyKind) {
    // Multiple workers expose missing initialization of otherwise idle siblings.
    const WORKERS: u32 = 2;

    with_watchdog(move || {
        let hardware = hardware();
        let policy = match kind {
            PolicyKind::Processor => WorkerPolicy::per_processor(nz!(WORKERS)),
            PolicyKind::MemoryRegion => WorkerPolicy::per_memory_region(nz!(WORKERS)),
            PolicyKind::System => WorkerPolicy::per_system(nz!(WORKERS)),
        };
        let (initialized, observations) = mpsc::channel();
        let pool = Pool::builder()
            .hardware(hardware.clone())
            .worker_policy(policy)
            .thread_init(|| panic!("replaced initializer"))
            .thread_init({
                let hardware = hardware.clone();
                move || {
                    assert!(!INITIALIZED.replace(true));
                    initialized
                        .send((thread::current().id(), affinity(&hardware)))
                        .unwrap();
                }
            })
            .build();

        let scheduler = pool.scheduler();
        assert!(pool.inner.worker_handles.lock().unwrap().is_empty());
        assert_eq!(
            observations.try_recv().unwrap_err(),
            mpsc::TryRecvError::Empty
        );

        // The shared region grows before work reaches a separate region.
        for processor in [0, 1, 3] {
            pin(&hardware, processor);
            assert!(block_on(scheduler.spawn(|| INITIALIZED.get())).unwrap());
            assert!(block_on(scheduler.spawn_urgent(|| INITIALIZED.get())).unwrap());
            let (result, received) = mpsc::channel();
            scheduler.spawn_and_forget({
                let result = result.clone();
                move || result.send(INITIALIZED.get()).unwrap()
            });
            scheduler.spawn_urgent_and_forget(move || {
                result.send(INITIALIZED.get()).unwrap();
            });
            assert!(received.recv().unwrap());
            assert!(received.recv().unwrap());
        }
        assert!(!INITIALIZED.get());

        // Joining also accounts for workers that never received a task.
        drop(pool);

        let mut threads = HashSet::new();
        let mut masks = Vec::new();
        for (id, mask) in observations.try_iter() {
            assert!(threads.insert(id));
            masks.push(mask);
        }

        let initial_processors = match kind {
            PolicyKind::Processor => vec![0, 1, 3],
            PolicyKind::MemoryRegion => vec![0, 3],
            PolicyKind::System => vec![0],
        };
        let expected: Vec<_> = initial_processors
            .into_iter()
            .flat_map(|processor| iter::repeat_n(BTreeSet::from([processor]), WORKERS as usize))
            .collect();
        masks.sort();
        assert_eq!(masks, expected);
    });
}

/// Submission proceeds while all initializers are blocked, but queued work remains untouched.
#[test]
fn initialization_is_concurrent_and_submission_does_not_wait() {
    // Sibling workers must both reach initialization before the caller releases them.
    const WORKERS: u32 = 2;

    with_watchdog(|| {
        let release = Arc::new(Barrier::new(WORKERS as usize + 1));
        let executed = Arc::new(AtomicBool::new(false));
        let pool = Pool::builder()
            .hardware(hardware())
            .worker_policy(WorkerPolicy::per_processor(nz!(WORKERS)))
            .thread_init({
                let release = Arc::clone(&release);
                move || {
                    release.wait();
                    INITIALIZED.set(true);
                }
            })
            .build();

        // Reject count mutations before starting callbacks with a fixed participant count.
        assert_eq!(pool.inner.policy.count().get(), WORKERS);
        let task = pool.scheduler().spawn({
            let executed = Arc::clone(&executed);
            move || {
                assert!(INITIALIZED.get());
                executed.store(true, Ordering::Release);
            }
        });
        assert!(!executed.load(Ordering::Acquire));

        release.wait();
        block_on(task).unwrap();
        assert!(executed.load(Ordering::Acquire));
    });
}

/// A callback can submit to its own destination without waiting on its startup acknowledgment.
#[test]
fn initializer_can_submit_to_its_pool() {
    with_watchdog(|| {
        let scheduler = Arc::new(OnceLock::<Scheduler>::new());
        let (result, received) = mpsc::channel();
        let pool = Pool::builder()
            .hardware(hardware())
            .worker_policy(WorkerPolicy::per_processor(nz!(1)))
            .thread_init({
                // A weak capture avoids retaining a scheduler through its own pool.
                let scheduler = Arc::downgrade(&scheduler);
                move || {
                    INITIALIZED.set(true);
                    let scheduler = scheduler.upgrade().unwrap();
                    scheduler.get().unwrap().spawn_and_forget({
                        let result = result.clone();
                        move || result.send(INITIALIZED.get()).unwrap()
                    });
                }
            })
            .build();

        scheduler.set(pool.scheduler()).unwrap();
        assert!(block_on(scheduler.get().unwrap().spawn(|| INITIALIZED.get())).unwrap());
        assert!(received.recv().unwrap());
    });
}

/// A callback failure cancels accepted work and permanently poisons every submission path.
#[test]
fn initialization_panic_stops_the_pool() {
    with_watchdog(|| {
        let (release, wait) = mpsc::channel();
        let pool = Pool::builder()
            .hardware(hardware())
            .worker_policy(WorkerPolicy::per_processor(nz!(1)))
            .thread_init({
                let wait = Mutex::new(wait);
                move || {
                    wait.lock().unwrap().recv().unwrap();
                    panic!("injected initialization failure");
                }
            })
            .build();

        // The controller releases exactly one initializer.
        assert_eq!(pool.inner.policy.count(), nz!(1));
        let scheduler = pool.scheduler();
        let executed = Arc::new(AtomicBool::new(false));
        let task = scheduler.spawn({
            let executed = Arc::clone(&executed);
            move || executed.store(true, Ordering::Relaxed)
        });

        release.send(()).unwrap();
        assert!(block_on(task).unwrap_err().is_cancel());
        assert!(!executed.load(Ordering::Relaxed));
        assert!(pool.inner.poisoned.load(Ordering::Acquire));

        assert_panics(|| drop(pool));
        assert_panics(|| scheduler.spawn(|| {}));
        assert_panics(|| scheduler.spawn_urgent(|| {}));
        assert_panics(|| scheduler.spawn_and_forget(|| {}));
        assert_panics(|| scheduler.spawn_urgent_and_forget(|| {}));
    });
}
