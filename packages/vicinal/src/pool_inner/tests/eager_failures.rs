use std::any::Any;
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::{panic, thread};

use many_cpus::SystemHardware;
use many_cpus::fake::HardwareBuilder;
use new_zealand::nz;
use testing::{assert_panics, with_watchdog};

use crate::{Pool, WorkerGroup, WorkerGroupId, WorkerGroups, WorkerPolicy, WorkerPolicyContext};

struct PreparationFailure;

struct StartupFailurePolicy {
    group: OnceLock<WorkerGroupId>,
    worker_observer: Arc<Mutex<Option<WorkerGroup>>>,
    ready: Mutex<Option<mpsc::Receiver<()>>>,
}

struct EagerCustomPolicy {
    group: OnceLock<WorkerGroupId>,
}

impl WorkerPolicy for EagerCustomPolicy {
    fn prepare(&mut self, context: WorkerPolicyContext<'_>) {
        let group = context.groups().add();
        self.group.set(group.id()).unwrap();
        thread::spawn({
            let group = group.clone();
            move || group.execute_worker()
        });
        group.wait_until_initialized(std::num::NonZero::new(1).unwrap());
    }

    fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
        *self.group.get().unwrap()
    }
}

impl WorkerPolicy for StartupFailurePolicy {
    fn prepare(&mut self, context: WorkerPolicyContext<'_>) {
        let group = context.groups().add();
        self.group.set(group.id()).unwrap();
        *self.worker_observer.lock().unwrap() = Some(group.clone());
        thread::spawn(move || group.execute_worker());
        self.ready.lock().unwrap().take().unwrap().recv().unwrap();
        panic::panic_any(PreparationFailure);
    }

    fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
        *self.group.get().unwrap()
    }
}

#[test]
fn policy_preparation_failure_stops_started_workers() {
    with_watchdog(|| {
        let observer = Arc::new(Mutex::new(None));
        let (ready, worker_ready) = mpsc::channel();
        let payload: Box<dyn Any + Send> = panic::catch_unwind(|| {
            _ = Pool::builder()
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
        assert!(group.is_empty());
    });
}

#[test]
fn eager_initializer_failure_reaches_builder() {
    struct InitializationFailure;

    with_watchdog(|| {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(2), nz!(1)));
        let processors = hardware.all_processors();
        let payload = panic::catch_unwind(|| {
            _ = Pool::builder()
                .hardware(hardware)
                .eager(processors)
                .worker_policy(crate::DefaultWorkerPolicy::per_system(nz!(2)))
                .thread_init(|| panic::panic_any(InitializationFailure))
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

    assert_panics(|| {
        _ = Pool::builder()
            .hardware(first)
            .eager(foreign.all_processors())
            .worker_policy(StartupFailurePolicy {
                group: OnceLock::new(),
                worker_observer: Arc::new(Mutex::new(None)),
                ready: Mutex::new(None),
            })
            .build();
    });
}

#[test]
fn custom_policy_can_wait_for_eager_initialization() {
    with_watchdog(|| {
        let initialized = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let pool = Pool::builder()
            .worker_policy(EagerCustomPolicy {
                group: OnceLock::new(),
            })
            .thread_init({
                let initialized = Arc::clone(&initialized);
                move || initialized.store(true, std::sync::atomic::Ordering::Release)
            })
            .build();

        assert!(initialized.load(std::sync::atomic::Ordering::Acquire));
        drop(pool);
    });
}
