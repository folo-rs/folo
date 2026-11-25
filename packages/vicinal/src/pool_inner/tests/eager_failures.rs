use std::any::Any;
use std::panic::{catch_unwind, panic_any};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::spawn;

use many_cpus::SystemHardware;
use many_cpus::fake::HardwareBuilder;
use new_zealand::nz;
use testing::{assert_panics, with_watchdog};

use crate::{Pool, PrepareContext, WorkerGroup, WorkerGroupId, WorkerGroups, WorkerPolicy};

struct PreparationFailure;

/// Stops a started worker and records preparation failure for cleanup-boundary tests.
struct StartupFailurePolicy {
    group: OnceLock<WorkerGroupId>,
    worker_observer: Arc<Mutex<Option<WorkerGroup>>>,
    ready: Mutex<Option<Receiver<()>>>,
}

impl WorkerPolicy for StartupFailurePolicy {
    fn prepare(&mut self, context: PrepareContext<'_>) {
        let group = context.groups().add();
        self.group.set(group.id()).unwrap();
        *self.worker_observer.lock().unwrap() = Some(group.clone());
        let worker = group.add_worker();
        _ = spawn(move || worker.run());
        self.ready.lock().unwrap().take().unwrap().recv().unwrap();
        panic_any(PreparationFailure);
    }

    fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
        *self.group.get().unwrap()
    }
}

/// Starts a failing worker, observes published failure, then returns from preparation.
struct ReturningFailurePolicy {
    failure: Mutex<Option<Receiver<()>>>,
}

impl WorkerPolicy for ReturningFailurePolicy {
    fn prepare(&mut self, context: PrepareContext<'_>) {
        let group = context.groups().add();
        let (failed, failure) = channel();
        assert!(
            group
                .state()
                .before_failure_exit
                .set(Box::new(move || failed.send(()).unwrap()))
                .is_ok()
        );
        *self.failure.lock().unwrap() = Some(failure);
        let worker = group.add_worker();
        _ = spawn(move || worker.run());
        self.failure.lock().unwrap().take().unwrap().recv().unwrap();
    }

    fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
        unreachable!("construction must reject the recorded worker failure")
    }
}

/// Reserves multiple custom workers during preparation for builder-owned readiness tracking.
struct EagerCustomPolicy {
    group: OnceLock<WorkerGroupId>,
}

impl WorkerPolicy for EagerCustomPolicy {
    fn prepare(&mut self, context: PrepareContext<'_>) {
        let group = context.groups().add();
        self.group.set(group.id()).unwrap();
        for _ in 0..2 {
            _ = spawn({
                let worker = group.add_worker();
                move || worker.run()
            });
        }
    }

    fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
        *self.group.get().unwrap()
    }
}

/// Records whether preparation can abandon a worker that will never run.
struct DroppedWorkerPolicy {
    group: OnceLock<WorkerGroupId>,
}

impl WorkerPolicy for DroppedWorkerPolicy {
    fn prepare(&mut self, context: PrepareContext<'_>) {
        let group = context.groups().add();
        self.group.set(group.id()).unwrap();
        drop(group.add_worker());
    }

    fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
        *self.group.get().unwrap()
    }
}

#[test]
fn policy_preparation_failure_stops_started_workers() {
    with_watchdog(|| {
        let observer = Arc::new(Mutex::new(None));
        let (ready, worker_ready) = channel();
        let payload: Box<dyn Any + Send> = catch_unwind(|| {
            _ = Pool::builder()
                .hardware(SystemHardware::fake(HardwareBuilder::from_counts(
                    nz!(1),
                    nz!(1),
                )))
                .worker_policy(StartupFailurePolicy {
                    group: OnceLock::new(),
                    worker_observer: Arc::clone(&observer),
                    ready: Mutex::new(Some(worker_ready)),
                })
                .thread_init(move || ready.send(()).unwrap())
                .build();
        })
        .unwrap_err();

        assert!(payload.is::<PreparationFailure>());
        let group = observer
            .lock()
            .unwrap()
            .take()
            .expect("preparation retained the created group for observation");
        assert_eq!(group.worker_count(), 0);
    });
}

#[test]
fn construction_rejects_failure_published_before_prepare_returns() {
    struct InitializationFailure;

    with_watchdog(|| {
        let payload = catch_unwind(|| {
            _ = Pool::builder()
                .hardware(SystemHardware::fake(HardwareBuilder::from_counts(
                    nz!(1),
                    nz!(1),
                )))
                .worker_policy(ReturningFailurePolicy {
                    failure: Mutex::new(None),
                })
                .thread_init(|| panic_any(InitializationFailure))
                .build();
        })
        .unwrap_err();

        assert!(payload.is::<InitializationFailure>());
    });
}

#[test]
fn eager_initializer_failure_reaches_builder() {
    struct InitializationFailure;

    with_watchdog(|| {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(2), nz!(1)));
        let processors = hardware.all_processors();
        let payload = catch_unwind(|| {
            _ = Pool::builder()
                .hardware(hardware)
                .eager(processors)
                .worker_policy(crate::BuiltInWorkerPolicy::per_system(nz!(2)))
                .thread_init(|| panic_any(InitializationFailure))
                .build();
        })
        .unwrap_err();

        assert!(payload.is::<InitializationFailure>());
    });
}

/// A custom policy receives eager selections only after pool-level provenance validation.
#[test]
fn custom_policy_rejects_foreign_eager_hardware() {
    let first = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
    let foreign = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
    let observer = Arc::new(Mutex::new(None));

    assert_panics(|| {
        _ = Pool::builder()
            .hardware(first)
            .eager(foreign.all_processors())
            .worker_policy(StartupFailurePolicy {
                group: OnceLock::new(),
                worker_observer: Arc::clone(&observer),
                ready: Mutex::new(None),
            })
            .build();
    });
    assert!(observer.lock().unwrap().is_none());
}

#[test]
fn builder_waits_for_every_custom_preparation_worker() {
    with_watchdog(|| {
        let initialized = Arc::new(AtomicUsize::new(0));
        let pool = Pool::builder()
            .hardware(SystemHardware::fake(HardwareBuilder::from_counts(
                nz!(1),
                nz!(1),
            )))
            .worker_policy(EagerCustomPolicy {
                group: OnceLock::new(),
            })
            .thread_init({
                let initialized = Arc::clone(&initialized);
                // The builder waits for the policy's reserved worker; this flag only observes
                // that every initializer ran before construction returned.
                move || {
                    initialized.fetch_add(1, Ordering::Release);
                }
            })
            .build();

        assert_eq!(initialized.load(Ordering::Acquire), 2);
        drop(pool);
    });
}

#[test]
fn dropping_a_preparation_worker_cancels_its_readiness_target() {
    with_watchdog(|| {
        let pool = Pool::builder()
            .hardware(SystemHardware::fake(HardwareBuilder::from_counts(
                nz!(1),
                nz!(1),
            )))
            .worker_policy(DroppedWorkerPolicy {
                group: OnceLock::new(),
            })
            .build();

        drop(pool);
    });
}
