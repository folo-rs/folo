use std::io::Error as IoError;
use std::panic::{AssertUnwindSafe, catch_unwind, panic_any};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::{JoinHandle, spawn};

use many_cpus::SystemHardware;
use many_cpus::fake::HardwareBuilder;
use new_zealand::nz;
use ohno::ErrorExt;
use testing::with_watchdog;

use crate::build_error::BuildPanicked;
use crate::worker_policy::{
    BindContext, BindError, BoundPolicy, Worker, WorkerGroup, WorkerGroupId, WorkerGroups,
    WorkerPolicy,
};
use crate::{BuiltInWorkerPolicy, EagerProcessorHardwareMismatch, Pool};

/// Distinguishes a panic from `bind()` from ordinary test synchronization signals.
struct BindingFailure;
/// Distinguishes a policy destructor panic from the binding error returned first.
struct PolicyDropFailure;

/// Distinguishes the expected binding signal from an unexpected worker return.
#[derive(Debug, Eq, PartialEq)]
enum BindingEvent {
    Ready,
    Failed,
    Returned,
}

/// Reports an ordinary binding error, optionally superseded by a policy destructor panic.
#[derive(Debug)]
struct FailingBindingPolicy {
    worker: Option<BindingWorker>,
    dropped: Option<Sender<()>>,
}

impl WorkerPolicy for FailingBindingPolicy {
    fn bind(mut self, context: BindContext<'_>) -> Result<impl BoundPolicy, BindError> {
        if let Some(worker) = self.worker.take() {
            worker.start(context.groups().add());
        }

        // Run the fixture's destructor before allocating the error so a panic cannot strand it
        // in the return place.
        drop(self);
        Err::<NeverBound, _>(BindError::new(IoError::other("injected binding failure")))
    }
}

/// Keeps an initialized worker participating until the controller releases cleanup.
#[derive(Debug)]
struct BindingWorker {
    started: Sender<(WorkerGroup, JoinHandle<()>)>,
    build_events: Sender<BuildEvent>,
    release: Receiver<()>,
}

impl BindingWorker {
    fn start(self, group: WorkerGroup) {
        let (events, progress) = channel();
        let release = Mutex::new(Some(self.release));
        assert!(
            group
                .state()
                .before_wait
                .set(Box::new({
                    let events = events.clone();
                    move |_, _| {
                        // Ordinary binding failure resolves readiness before waiting for
                        // participation. Parking in the initializer would block that first wait.
                        if let Some(release) = release.lock().unwrap().take() {
                            events.send(BindingEvent::Ready).unwrap();
                            _ = release.recv();
                        }
                    }
                }))
                .is_ok()
        );
        group.observe_worker_wait(self.build_events, BuildEvent::Waiting);
        let worker = group.add_worker();
        self.started
            .send((
                group,
                spawn(move || {
                    let result = catch_unwind(|| worker.run());
                    _ = events.send(BindingEvent::Returned);
                    result.unwrap();
                }),
            ))
            .unwrap();
        // An early worker return also releases binding; the controller rejects a build
        // that returns without observing live participation.
        _ = progress.recv().unwrap();
    }
}

impl Drop for FailingBindingPolicy {
    fn drop(&mut self) {
        if let Some(dropped) = &self.dropped {
            dropped.send(()).unwrap();
            panic_any(PolicyDropFailure);
        }
    }
}

/// A policy that returns a binding error cannot produce a route.
#[derive(Debug)]
enum NeverBound {}

impl BoundPolicy for NeverBound {
    #[expect(
        clippy::uninhabited_references,
        reason = "The binding fixture only returns Err, so no reference can reach this match."
    )]
    fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
        match *self {}
    }
}

/// Orders entry to the participation wait against completion of construction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BuildEvent {
    Waiting,
    Returned,
}

/// Panics if cleanup drops the worker initializer's capture.
struct PanickingInitializerDrop {
    dropped: Sender<()>,
}

impl Drop for PanickingInitializerDrop {
    fn drop(&mut self) {
        self.dropped.send(()).unwrap();
        panic_any(PolicyDropFailure);
    }
}

/// Panics while dropping the bound policy after a worker failure.
#[derive(Debug)]
struct PanickingBoundDropPolicy {
    dropped: Sender<()>,
}

impl WorkerPolicy for PanickingBoundDropPolicy {
    fn bind(self, context: BindContext<'_>) -> Result<impl BoundPolicy, BindError> {
        let worker = context.groups().add().add_worker();
        _ = spawn(move || worker.run());
        Ok(self)
    }
}

impl BoundPolicy for PanickingBoundDropPolicy {
    fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
        unreachable!("construction rejects the worker failure")
    }
}

impl Drop for PanickingBoundDropPolicy {
    fn drop(&mut self) {
        self.dropped.send(()).unwrap();
        panic_any(PolicyDropFailure);
    }
}

/// Stops a started worker and records binding failure for cleanup-boundary tests.
#[derive(Debug)]
struct StartupFailurePolicy {
    worker_observer: Arc<Mutex<Option<WorkerGroup>>>,
    ready: Mutex<Option<(Sender<BindingEvent>, Receiver<BindingEvent>)>>,
}

impl WorkerPolicy for StartupFailurePolicy {
    fn bind(self, context: BindContext<'_>) -> Result<impl BoundPolicy, BindError> {
        let group = context.groups().add();
        *self.worker_observer.lock().unwrap() = Some(group.clone());
        let (returned, ready) = {
            let mut ready = self.ready.lock().unwrap();
            ready.take().unwrap()
        };
        let worker = group.add_worker();
        _ = spawn(move || {
            worker.run();
            _ = returned.send(BindingEvent::Returned);
        });
        assert_eq!(ready.recv().unwrap(), BindingEvent::Ready);
        panic_any(BindingFailure);
        #[allow(
            unreachable_code,
            reason = "panic_any proves the failure path while this gives the opaque return a concrete type"
        )]
        Ok(self)
    }
}

impl BoundPolicy for StartupFailurePolicy {
    fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
        unreachable!("construction must reject the binding failure")
    }
}

/// Starts a failing worker, observes published failure, then returns from binding.
#[derive(Debug)]
struct ReturningFailurePolicy;

impl WorkerPolicy for ReturningFailurePolicy {
    fn bind(self, context: BindContext<'_>) -> Result<impl BoundPolicy, BindError> {
        let group = context.groups().add();
        let (events, failure) = channel();
        assert!(
            group
                .state()
                .before_failure_exit
                .set(Box::new({
                    let events = events.clone();
                    move || events.send(BindingEvent::Failed).unwrap()
                }))
                .is_ok()
        );
        let worker = group.add_worker();
        _ = spawn(move || {
            worker.run();
            _ = events.send(BindingEvent::Returned);
        });
        assert_eq!(failure.recv().unwrap(), BindingEvent::Failed);
        Ok(self)
    }
}

impl BoundPolicy for ReturningFailurePolicy {
    fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
        unreachable!("construction must reject the recorded worker failure")
    }
}

/// Reserves multiple custom workers during binding for builder-owned readiness tracking.
#[derive(Debug)]
struct EagerCustomPolicy;

impl WorkerPolicy for EagerCustomPolicy {
    fn bind(self, context: BindContext<'_>) -> Result<impl BoundPolicy, BindError> {
        let group = context.groups().add();
        for _ in 0..2 {
            _ = spawn({
                let worker = group.add_worker();
                move || worker.run()
            });
        }
        Ok(BoundCustomPolicy { group: group.id() })
    }
}

/// Retains the route independently of how the test resolves its binding reservations.
#[derive(Debug)]
struct BoundCustomPolicy {
    group: WorkerGroupId,
}

impl BoundPolicy for BoundCustomPolicy {
    fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
        self.group
    }
}

/// Records whether binding can abandon a worker that will never run.
#[derive(Debug)]
struct DroppedWorkerPolicy {
    reserved: Sender<Worker>,
    waiting: Sender<()>,
}

impl WorkerPolicy for DroppedWorkerPolicy {
    fn bind(self, context: BindContext<'_>) -> Result<impl BoundPolicy, BindError> {
        let group = context.groups().add();
        group.observe_binding_wait(self.waiting);
        self.reserved.send(group.add_worker()).unwrap();
        Ok(BoundCustomPolicy { group: group.id() })
    }
}

/// Retains a built-in eager group for observing construction failure cleanup.
#[derive(Debug)]
struct ObservedEagerPolicy {
    inner: BuiltInWorkerPolicy,
    observer: Arc<Mutex<Option<WorkerGroup>>>,
}

impl WorkerPolicy for ObservedEagerPolicy {
    fn bind(self, context: BindContext<'_>) -> Result<impl BoundPolicy, BindError> {
        let inner = self.inner.bind(context)?;
        *self.observer.lock().unwrap() = context.groups().iter().next();
        Ok(ObservedBoundPolicy {
            inner: Box::new(inner),
        })
    }
}

#[derive(Debug)]
struct ObservedBoundPolicy {
    inner: Box<dyn BoundPolicy>,
}

impl BoundPolicy for ObservedBoundPolicy {
    fn select_group(&self, groups: &WorkerGroups) -> WorkerGroupId {
        self.inner.select_group(groups)
    }
}

#[test]
fn policy_binding_failure_stops_started_workers() {
    with_watchdog(|| {
        let observer = Arc::new(Mutex::new(None));
        let (ready, worker_ready) = channel();
        let error = Pool::builder()
            .hardware(SystemHardware::fake(HardwareBuilder::from_counts(
                nz!(1),
                nz!(1),
            )))
            .worker_policy(StartupFailurePolicy {
                worker_observer: Arc::clone(&observer),
                ready: Mutex::new(Some((ready.clone(), worker_ready))),
            })
            .thread_init(move || ready.send(BindingEvent::Ready).unwrap())
            .build()
            .unwrap_err();

        assert!(error.find_source::<BuildPanicked>().is_some());
        let group = observer
            .lock()
            .unwrap()
            .take()
            .expect("binding retained the created group for observation");
        assert_eq!(group.worker_count(), 0);
    });
}

#[test]
fn policy_drop_failure_waits_for_binding_workers() {
    assert_failed_binding_waits_for_workers(true);
}

#[test]
fn ordinary_binding_error_waits_for_binding_workers() {
    assert_failed_binding_waits_for_workers(false);
}

/// Keeps one initialized worker in participation until construction reaches its cleanup wait.
fn assert_failed_binding_waits_for_workers(panic_on_drop: bool) {
    with_watchdog(move || {
        let (release_worker, worker_release) = channel();
        let (events, build_events) = channel();
        let (started, worker_observer) = channel();
        let (dropped, policy_dropped) = channel();
        let construction = spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(|| {
                Pool::builder()
                    .hardware(SystemHardware::fake(HardwareBuilder::from_counts(
                        nz!(1),
                        nz!(1),
                    )))
                    .worker_policy(FailingBindingPolicy {
                        worker: Some(BindingWorker {
                            started,
                            build_events: events.clone(),
                            release: worker_release,
                        }),
                        dropped: panic_on_drop.then_some(dropped),
                    })
                    .build()
            }));
            events.send(BuildEvent::Returned).unwrap();
            result.map(|result| result.map(drop))
        });

        // A return before the wait is a failing outcome, not a reason to leave a worker gated.
        let first = build_events.recv();
        let released = release_worker.send(());
        let construction = construction.join();
        let (group, worker) = worker_observer.recv().unwrap();
        let worker = worker.join();

        assert_eq!(first, Ok(BuildEvent::Waiting));
        released.unwrap();
        let error = construction.unwrap().unwrap().unwrap_err();
        worker.unwrap();
        assert_eq!(group.worker_count(), 0);
        if panic_on_drop {
            policy_dropped.recv().unwrap();
            assert!(error.find_source::<BuildPanicked>().is_some());
        } else {
            assert!(error.find_source::<BuildPanicked>().is_none());
            assert!(error.find_source::<BindError>().is_some());
            assert!(error.find_source::<IoError>().is_some());
        }
    });
}

#[test]
fn eager_rejection_prioritizes_unbound_policy_drop_panic() {
    let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
    let foreign = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
    let (dropped, policy_dropped) = channel();

    let builder = Pool::builder()
        .hardware(hardware)
        .eager(foreign.all_processors())
        .worker_policy(FailingBindingPolicy {
            worker: None,
            dropped: Some(dropped),
        });

    let payload = catch_unwind(AssertUnwindSafe(|| builder.build())).unwrap_err();
    policy_dropped.recv().unwrap();
    assert!(payload.is::<PolicyDropFailure>());
}

#[test]
fn eager_rejection_prioritizes_initializer_drop_panic() {
    let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
    let foreign = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
    let (dropped, initializer_dropped) = channel();
    let marker = PanickingInitializerDrop { dropped };

    let builder = Pool::builder()
        .hardware(hardware)
        .eager(foreign.all_processors())
        .thread_init(move || {
            _ = &marker;
        });

    let payload = catch_unwind(AssertUnwindSafe(|| builder.build())).unwrap_err();
    initializer_dropped.recv().unwrap();
    assert!(payload.is::<PolicyDropFailure>());
}

#[test]
fn worker_failure_prioritizes_bound_policy_drop_panic() {
    /// Distinguishes the worker initializer failure from policy cleanup.
    struct InitializationFailure;

    let (dropped, policy_dropped) = channel();
    let builder = Pool::builder()
        .hardware(SystemHardware::fake(HardwareBuilder::from_counts(
            nz!(1),
            nz!(1),
        )))
        .worker_policy(PanickingBoundDropPolicy { dropped })
        .thread_init(|| panic_any(InitializationFailure));

    let payload = catch_unwind(AssertUnwindSafe(|| builder.build())).unwrap_err();
    policy_dropped.recv().unwrap();
    assert!(payload.is::<PolicyDropFailure>());
}

#[test]
fn worker_failure_prioritizes_initializer_drop_panic() {
    /// Distinguishes the worker initializer failure from destructor cleanup.
    struct InitializationFailure;

    let (dropped, initializer_dropped) = channel();
    let marker = PanickingInitializerDrop { dropped };
    let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
    let eager = hardware.all_processors();
    // A surviving group handle must not delay the failed builder's initializer cleanup.
    let observer = Arc::new(Mutex::new(None));
    let builder = Pool::builder()
        .hardware(hardware)
        .eager(eager)
        .worker_policy(ObservedEagerPolicy {
            inner: BuiltInWorkerPolicy::per_processor(nz!(1)),
            observer: Arc::clone(&observer),
        })
        .thread_init(move || {
            _ = &marker;
            panic_any(InitializationFailure);
        });

    let payload = catch_unwind(AssertUnwindSafe(|| builder.build())).unwrap_err();
    initializer_dropped.recv().unwrap();
    drop(observer);
    assert!(payload.is::<PolicyDropFailure>());
}

#[test]
fn construction_rejects_failure_published_before_bind_returns() {
    /// Distinguishes the worker initializer panic from construction or cleanup failures.
    struct InitializationFailure;

    with_watchdog(|| {
        let error = Pool::builder()
            .hardware(SystemHardware::fake(HardwareBuilder::from_counts(
                nz!(1),
                nz!(1),
            )))
            .worker_policy(ReturningFailurePolicy)
            .thread_init(|| panic_any(InitializationFailure))
            .build()
            .unwrap_err();

        assert!(error.find_source::<BuildPanicked>().is_some());
    });
}

#[test]
fn eager_partial_initializer_failure_leaves_no_participating_workers() {
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
        let error = Pool::builder()
            .hardware(hardware)
            .eager(processors)
            .worker_policy(ObservedEagerPolicy {
                inner: BuiltInWorkerPolicy::per_system(nz!(2)),
                observer: Arc::clone(&observer),
            })
            .thread_init({
                let initialized = Arc::clone(&initialized);
                // This counter only assigns roles; channels and cleanup coordinate the workers.
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
            .unwrap_err();

        assert!(error.find_source::<BuildPanicked>().is_some());
        first_completion.recv().unwrap();
        assert_eq!(initialized.load(Ordering::Relaxed), 2);
        let group = observer.lock().unwrap().take().unwrap();
        assert_eq!(group.worker_count(), 0);
    });
}

/// A custom policy receives eager selections only after pool-level provenance validation.
#[test]
fn builder_rejects_foreign_eager_hardware_before_policy_binding() {
    let first = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
    let foreign = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
    let observer = Arc::new(Mutex::new(None));

    let error = Pool::builder()
        .hardware(first)
        .eager(foreign.all_processors())
        .worker_policy(StartupFailurePolicy {
            worker_observer: Arc::clone(&observer),
            ready: Mutex::new(None),
        })
        .build()
        .unwrap_err();
    assert!(
        error
            .find_source::<EagerProcessorHardwareMismatch>()
            .is_some()
    );
    // Equal processor counts make hardware identity the only mismatch. The observer proves
    // construction rejected that mismatch before entering the policy's failing bind call.
    assert!(observer.lock().unwrap().is_none());
}

#[test]
fn builder_waits_for_every_custom_binding_worker() {
    with_watchdog(|| {
        let initialized = Arc::new(AtomicUsize::new(0));
        let pool = Pool::builder()
            .hardware(SystemHardware::fake(HardwareBuilder::from_counts(
                nz!(1),
                nz!(1),
            )))
            .worker_policy(EagerCustomPolicy)
            .thread_init({
                let initialized = Arc::clone(&initialized);
                // The reservation mutex and condition variable provide the handoff. This
                // counter only observes completed initializers.
                move || {
                    initialized.fetch_add(1, Ordering::Relaxed);
                }
            })
            .build()
            .unwrap();

        assert_eq!(initialized.load(Ordering::Relaxed), 2);
        drop(pool);
    });
}

#[test]
fn dropping_a_binding_worker_cancels_its_readiness_target() {
    with_watchdog(|| {
        let (reserved, reservation) = channel();
        let (waiting, wait) = channel();
        let builder = spawn(move || {
            Pool::builder()
                .hardware(SystemHardware::fake(HardwareBuilder::from_counts(
                    nz!(1),
                    nz!(1),
                )))
                .worker_policy(DroppedWorkerPolicy { reserved, waiting })
                .build()
        });

        let worker = reservation.recv().unwrap();
        wait.recv().unwrap();
        drop(worker);
        drop(builder.join().unwrap().unwrap());
    });
}
