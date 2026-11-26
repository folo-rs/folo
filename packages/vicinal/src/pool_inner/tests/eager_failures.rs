use std::any::Any;
use std::io;
use std::panic::{catch_unwind, panic_any};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::spawn;

use many_cpus::SystemHardware;
use many_cpus::fake::HardwareBuilder;
use new_zealand::nz;
use testing::{assert_panics, with_watchdog};

use crate::{
    BoundWorkerPolicy, BuiltInWorkerPolicy, Pool, PrepareContext, PrepareError, WorkerGroup,
    WorkerGroupId, WorkerGroups, WorkerPolicy,
};

/// Distinguishes a panic from `bind()` from ordinary test synchronization signals.
struct PreparationFailure;
/// Distinguishes a policy destructor panic from the preparation error returned first.
struct PolicyDropFailure;

/// Distinguishes the expected preparation signal from an unexpected worker return.
#[derive(Debug, Eq, PartialEq)]
enum PreparationEvent {
    Ready,
    Failed,
    Returned,
}

/// Stops a started worker and records preparation failure for cleanup-boundary tests.
#[derive(Debug)]
struct StartupFailurePolicy {
    worker_observer: Arc<Mutex<Option<WorkerGroup>>>,
    ready: Mutex<Option<(Sender<PreparationEvent>, Receiver<PreparationEvent>)>>,
}

/// Reports preparation failure and observes whether destructor failure waits for worker exit.
#[derive(Debug)]
struct PanickingDropPolicy {
    worker_observer: Arc<Mutex<Option<WorkerGroup>>>,
    initializer: Mutex<Receiver<()>>,
    dropped: Sender<()>,
}

impl WorkerPolicy for PanickingDropPolicy {
    fn bind(self, context: PrepareContext<'_>) -> Result<impl BoundWorkerPolicy, PrepareError> {
        let group = context.groups().add();
        *self.worker_observer.lock().unwrap() = Some(group.clone());
        let worker = group.add_worker();
        _ = spawn(move || worker.run());
        self.initializer.lock().unwrap().recv().unwrap();

        Err::<Self, _>(PrepareError::new(io::Error::other(
            "injected preparation failure",
        )))
    }
}

impl BoundWorkerPolicy for PanickingDropPolicy {
    fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
        unreachable!("construction must reject the preparation failure")
    }
}

impl Drop for PanickingDropPolicy {
    fn drop(&mut self) {
        self.dropped.send(()).unwrap();
        panic_any(PolicyDropFailure);
    }
}

impl WorkerPolicy for StartupFailurePolicy {
    fn bind(self, context: PrepareContext<'_>) -> Result<impl BoundWorkerPolicy, PrepareError> {
        let group = context.groups().add();
        *self.worker_observer.lock().unwrap() = Some(group.clone());
        let (returned, ready) = {
            let mut ready = self.ready.lock().unwrap();
            ready.take().unwrap()
        };
        let worker = group.add_worker();
        _ = spawn(move || {
            worker.run();
            _ = returned.send(PreparationEvent::Returned);
        });
        assert_eq!(ready.recv().unwrap(), PreparationEvent::Ready);
        panic_any(PreparationFailure);
        #[allow(
            unreachable_code,
            reason = "panic_any proves the failure path while this gives the opaque return a concrete type"
        )]
        Ok(self)
    }
}

impl BoundWorkerPolicy for StartupFailurePolicy {
    fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
        unreachable!("construction must reject the preparation failure")
    }
}

/// Starts a failing worker, observes published failure, then returns from preparation.
#[derive(Debug)]
struct ReturningFailurePolicy;

impl WorkerPolicy for ReturningFailurePolicy {
    fn bind(self, context: PrepareContext<'_>) -> Result<impl BoundWorkerPolicy, PrepareError> {
        let group = context.groups().add();
        let (failed, failure) = channel();
        let returned = failed.clone();
        assert!(
            group
                .state()
                .before_failure_exit
                .set(Box::new(move || {
                    failed.send(PreparationEvent::Failed).unwrap();
                }))
                .is_ok()
        );
        let worker = group.add_worker();
        _ = spawn(move || {
            worker.run();
            _ = returned.send(PreparationEvent::Returned);
        });
        assert_eq!(failure.recv().unwrap(), PreparationEvent::Failed);
        Ok(self)
    }
}

impl BoundWorkerPolicy for ReturningFailurePolicy {
    fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
        unreachable!("construction must reject the recorded worker failure")
    }
}

/// Reserves multiple custom workers during preparation for builder-owned readiness tracking.
#[derive(Debug)]
struct EagerCustomPolicy {
    group: OnceLock<WorkerGroupId>,
}

impl WorkerPolicy for EagerCustomPolicy {
    fn bind(self, context: PrepareContext<'_>) -> Result<impl BoundWorkerPolicy, PrepareError> {
        let group = context.groups().add();
        self.group.set(group.id()).unwrap();
        for _ in 0..2 {
            _ = spawn({
                let worker = group.add_worker();
                move || worker.run()
            });
        }
        Ok(self)
    }
}

impl BoundWorkerPolicy for EagerCustomPolicy {
    fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
        *self.group.get().unwrap()
    }
}

/// Records whether preparation can abandon a worker that will never run.
#[derive(Debug)]
struct DroppedWorkerPolicy {
    group: OnceLock<WorkerGroupId>,
}

impl WorkerPolicy for DroppedWorkerPolicy {
    fn bind(self, context: PrepareContext<'_>) -> Result<impl BoundWorkerPolicy, PrepareError> {
        let group = context.groups().add();
        self.group.set(group.id()).unwrap();
        drop(group.add_worker());
        Ok(self)
    }
}

impl BoundWorkerPolicy for DroppedWorkerPolicy {
    fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
        *self.group.get().unwrap()
    }
}

/// Retains a built-in eager group for observing construction failure cleanup.
#[derive(Debug)]
struct ObservedEagerPolicy {
    inner: BuiltInWorkerPolicy,
    observer: Arc<Mutex<Option<WorkerGroup>>>,
}

impl WorkerPolicy for ObservedEagerPolicy {
    fn bind(self, context: PrepareContext<'_>) -> Result<impl BoundWorkerPolicy, PrepareError> {
        let inner = self.inner.bind(context)?;
        *self.observer.lock().unwrap() = context.groups().iter().next();
        Ok(ObservedBoundPolicy {
            inner: Box::new(inner),
        })
    }
}

#[derive(Debug)]
struct ObservedBoundPolicy {
    inner: Box<dyn BoundWorkerPolicy>,
}

impl BoundWorkerPolicy for ObservedBoundPolicy {
    fn select_group(&self, groups: &WorkerGroups) -> WorkerGroupId {
        self.inner.select_group(groups)
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
                    worker_observer: Arc::clone(&observer),
                    ready: Mutex::new(Some((ready.clone(), worker_ready))),
                })
                .thread_init(move || ready.send(PreparationEvent::Ready).unwrap())
                .build()
                .unwrap();
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
fn policy_drop_failure_waits_for_preparation_workers() {
    with_watchdog(|| {
        let observer = Arc::new(Mutex::new(None));
        let (initializer_entered, worker_initialized) = channel();
        let (release_initializer, initializer_release) = channel();
        let initializer_release = Mutex::new(initializer_release);
        let (dropped, policy_dropped) = channel();
        let construction = spawn({
            let observer = Arc::clone(&observer);
            move || {
                _ = Pool::builder()
                    .hardware(SystemHardware::fake(HardwareBuilder::from_counts(
                        nz!(1),
                        nz!(1),
                    )))
                    .worker_policy(PanickingDropPolicy {
                        worker_observer: observer,
                        initializer: Mutex::new(worker_initialized),
                        dropped,
                    })
                    .thread_init(move || {
                        initializer_entered.send(()).unwrap();
                        initializer_release.lock().unwrap().recv().unwrap();
                    })
                    .build()
                    .unwrap();
            }
        });

        policy_dropped.recv().unwrap();
        assert!(!construction.is_finished());
        release_initializer.send(()).unwrap();

        let payload = construction.join().unwrap_err();
        assert!(payload.is::<PolicyDropFailure>());
        let group = observer.lock().unwrap().take().unwrap();
        assert_eq!(group.worker_count(), 0);
    });
}

#[test]
fn construction_rejects_failure_published_before_prepare_returns() {
    /// Distinguishes the worker initializer panic from construction or cleanup failures.
    struct InitializationFailure;

    with_watchdog(|| {
        let payload = catch_unwind(|| {
            _ = Pool::builder()
                .hardware(SystemHardware::fake(HardwareBuilder::from_counts(
                    nz!(1),
                    nz!(1),
                )))
                .worker_policy(ReturningFailurePolicy)
                .thread_init(|| panic_any(InitializationFailure))
                .build()
                .unwrap();
        })
        .unwrap_err();

        assert!(payload.is::<InitializationFailure>());
    });
}

#[test]
fn eager_partial_initializer_failure_waits_for_started_worker() {
    /// Distinguishes the eager worker initializer panic from construction coordination.
    struct InitializationFailure;

    with_watchdog(|| {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(2), nz!(1)));
        let processors = hardware.all_processors();
        let initialized = Arc::new(AtomicUsize::new(0));
        let observer = Arc::new(Mutex::new(None));
        let (first_entered, first_entry) = channel();
        let (release_first, wait_for_release) = channel();
        let (first_completed, first_completion) = channel();
        let first_entry = Mutex::new(first_entry);
        let wait_for_release = Mutex::new(wait_for_release);
        let payload = catch_unwind(|| {
            _ = Pool::builder()
                .hardware(hardware)
                .eager(processors)
                .worker_policy(ObservedEagerPolicy {
                    inner: BuiltInWorkerPolicy::per_system(nz!(2)),
                    observer: Arc::clone(&observer),
                })
                .thread_init({
                    let initialized = Arc::clone(&initialized);
                    move || match initialized.fetch_add(1, Ordering::Relaxed) {
                        0 => {
                            first_entered.send(()).unwrap();
                            wait_for_release.lock().unwrap().recv().unwrap();
                            first_completed.send(()).unwrap();
                        }
                        1 => {
                            first_entry.lock().unwrap().recv().unwrap();
                            release_first.send(()).unwrap();
                            panic_any(InitializationFailure);
                        }
                        _ => unreachable!("the eager fixture starts exactly two workers"),
                    }
                })
                .build()
                .unwrap();
        })
        .unwrap_err();

        assert!(payload.is::<InitializationFailure>());
        first_completion.recv().unwrap();
        assert_eq!(initialized.load(Ordering::Relaxed), 2);
        let group = observer.lock().unwrap().take().unwrap();
        assert_eq!(group.worker_count(), 0);
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
                worker_observer: Arc::clone(&observer),
                ready: Mutex::new(None),
            })
            .build()
            .unwrap();
    });
    // Equal processor counts make hardware identity the only mismatch. The observer proves
    // construction rejected that mismatch before entering the policy's failing preparation.
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
            .build()
            .unwrap();

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
            .build()
            .unwrap();

        drop(pool);
    });
}
