//! Application-controlled growth through the public pool and scheduling APIs.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;

use futures::executor::block_on;
use many_cpus::SystemHardware;
use many_cpus::fake::HardwareBuilder;
use new_zealand::nz;
use testing::with_watchdog;
use vicinal::{Pool, ScalingPolicy, ScalingState, WorkerPolicy};

testing::set_allocator!();
testing::init_tracing!();

#[derive(Clone)]
struct Policy {
    calls: Arc<AtomicUsize>,
    allowed: Arc<AtomicBool>,
}

impl Policy {
    fn new(allowed: bool) -> Self {
        Self {
            calls: Arc::new(AtomicUsize::new(0)),
            allowed: Arc::new(AtomicBool::new(allowed)),
        }
    }
}

impl ScalingPolicy for Policy {
    fn start_worker(&self, state: &ScalingState) -> bool {
        assert!(state.workers() < state.max_workers());
        assert!(state.queued_tasks() > 0);
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.allowed.load(Ordering::SeqCst)
    }
}

fn hardware() -> SystemHardware {
    SystemHardware::fake(HardwareBuilder::from_counts(nz!(1_usize), nz!(1_usize)))
}

#[test]
fn grows_one_at_a_time_and_stops_calling_policy_at_maximum() {
    const MAXIMUM: usize = 3;

    with_watchdog(|| {
        for workers in [
            WorkerPolicy::per_processor(nz!(3_u32)),
            WorkerPolicy::per_memory_region(nz!(3_u32)),
            WorkerPolicy::per_system(nz!(3_u32)),
        ] {
            let hardware = hardware();
            let policy = Policy::new(true);
            let initialized = Arc::new(AtomicUsize::new(0));
            let pool = Pool::builder()
                .hardware(hardware.clone())
                .worker_policy(workers)
                .scaling_policy(policy.clone())
                .eager(hardware.all_processors().clone())
                .thread_init({
                    let initialized = Arc::clone(&initialized);
                    move || {
                        initialized.fetch_add(1, Ordering::SeqCst);
                    }
                })
                .build();
            assert_eq!(initialized.load(Ordering::SeqCst), 0);
            assert_eq!(policy.calls.load(Ordering::SeqCst), 0);

            let scheduler = pool.scheduler();
            let (started, ready) = channel();
            // Dropping release senders also unblocks tasks if an assertion fails.
            let mut releases = Vec::new();
            let mut handles = Vec::new();
            for expected in 1..=MAXIMUM {
                let (release, released) = channel::<()>();
                releases.push(release);
                handles.push(scheduler.spawn({
                    let started = started.clone();
                    move || {
                        started.send(()).unwrap();
                        _ = released.recv();
                    }
                }));
                ready.recv().unwrap();
                assert_eq!(initialized.load(Ordering::SeqCst), expected);
                assert_eq!(policy.calls.load(Ordering::SeqCst), expected);
            }

            let regular = scheduler.spawn(|| 42);
            let urgent = scheduler.spawn_urgent(|| 43);
            let (finished, completed) = channel();
            scheduler.spawn_and_forget({
                let finished = finished.clone();
                move || finished.send(()).unwrap()
            });
            scheduler.spawn_urgent_and_forget(move || finished.send(()).unwrap());
            assert_eq!(policy.calls.load(Ordering::SeqCst), MAXIMUM);
            assert_eq!(initialized.load(Ordering::SeqCst), MAXIMUM);

            drop(releases);
            for handle in handles {
                block_on(handle).unwrap();
            }
            assert_eq!(block_on(regular).unwrap(), 42);
            assert_eq!(block_on(urgent).unwrap(), 43);
            completed.recv().unwrap();
            completed.recv().unwrap();
            drop(pool);
            assert_eq!(policy.calls.load(Ordering::SeqCst), MAXIMUM);
            assert_eq!(initialized.load(Ordering::SeqCst), MAXIMUM);
        }
    });
}

#[test]
fn policy_can_decline_the_first_worker_and_allow_a_later_submission() {
    with_watchdog(|| {
        let policy = Policy::new(false);
        let initialized = Arc::new(AtomicUsize::new(0));
        let pool = Pool::builder()
            .hardware(hardware())
            .worker_policy(WorkerPolicy::per_system(nz!(1_u32)))
            .scaling_policy(policy.clone())
            .thread_init({
                let initialized = Arc::clone(&initialized);
                move || {
                    initialized.fetch_add(1, Ordering::SeqCst);
                }
            })
            .build();
        let scheduler = pool.scheduler();
        let first = scheduler.spawn(|| 42);
        assert_eq!(policy.calls.load(Ordering::SeqCst), 1);
        assert_eq!(initialized.load(Ordering::SeqCst), 0);

        policy.allowed.store(true, Ordering::SeqCst);
        let second = scheduler.spawn_urgent(|| 43);
        assert_eq!(block_on(first).unwrap(), 42);
        assert_eq!(block_on(second).unwrap(), 43);
        assert_eq!(policy.calls.load(Ordering::SeqCst), 2);
        assert_eq!(initialized.load(Ordering::SeqCst), 1);
    });
}

#[test]
fn queued_burst_waits_for_initialization_then_grows_to_the_ceiling() {
    const MAXIMUM: usize = 3;

    with_watchdog(|| {
        let policy = Policy::new(true);
        let (initializing, initialized) = channel();
        let (initialize_worker, initialization) = channel();
        let initialization = Mutex::new(Some(initialization));
        let pool = Pool::builder()
            .hardware(hardware())
            .worker_policy(WorkerPolicy::per_system(nz!(3_u32)))
            .scaling_policy(policy.clone())
            .thread_init(move || {
                let initialization = initialization.lock().unwrap().take();
                if let Some(initialization) = initialization {
                    initializing.send(()).unwrap();
                    _ = initialization.recv();
                }
            })
            .build();
        // Release initialization before pool destruction even when an assertion fails.
        let initialize = initialize_worker;
        let scheduler = pool.scheduler();
        let (started, ready) = channel();
        let mut releases = Vec::new();
        let mut handles = Vec::new();
        for index in 0..MAXIMUM {
            let (release, released) = channel::<()>();
            releases.push(release);
            handles.push(scheduler.spawn({
                let started = started.clone();
                move || {
                    started.send(()).unwrap();
                    _ = released.recv();
                }
            }));
            if index == 0 {
                initialized.recv().unwrap();
            }
        }
        assert_eq!(policy.calls.load(Ordering::SeqCst), 1);
        initialize.send(()).unwrap();
        for _ in 0..MAXIMUM {
            ready.recv().unwrap();
        }
        assert_eq!(policy.calls.load(Ordering::SeqCst), MAXIMUM);
        drop(releases);
        for handle in handles {
            block_on(handle).unwrap();
        }
    });
}

#[test]
fn concurrent_submissions_cannot_exceed_the_ceiling() {
    const MAXIMUM: usize = 3;
    const SUBMISSIONS: usize = 8;

    with_watchdog(|| {
        let policy = Policy::new(true);
        let pool = Pool::builder()
            .hardware(hardware())
            .worker_policy(WorkerPolicy::per_system(nz!(3_u32)))
            .scaling_policy(policy.clone())
            .build();
        let scheduler = pool.scheduler();
        let barrier = Barrier::new(SUBMISSIONS);
        let (started, ready) = channel();
        let tasks = thread::scope(|scope| {
            let producers = (0..SUBMISSIONS)
                .map(|_| {
                    let barrier = &barrier;
                    let scheduler = &scheduler;
                    let started = started.clone();
                    scope.spawn(move || {
                        let (release, released) = channel::<()>();
                        barrier.wait();
                        let handle = scheduler.spawn(move || {
                            started.send(()).unwrap();
                            _ = released.recv();
                        });
                        (release, handle)
                    })
                })
                .collect::<Vec<_>>();
            producers
                .into_iter()
                .map(|producer| producer.join().unwrap())
                .collect::<Vec<_>>()
        });
        for _ in 0..MAXIMUM {
            ready.recv().unwrap();
        }
        assert_eq!(policy.calls.load(Ordering::SeqCst), MAXIMUM);
        let handles = tasks
            .into_iter()
            .map(|(release, handle)| {
                drop(release);
                handle
            })
            .collect::<Vec<_>>();
        for handle in handles {
            block_on(handle).unwrap();
        }
        assert_eq!(policy.calls.load(Ordering::SeqCst), MAXIMUM);
    });
}
