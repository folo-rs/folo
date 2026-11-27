#![expect(
    missing_docs,
    reason = "This is a test file, documentation is not required."
)]

use std::error::Error;
use std::io::Error as IoError;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Mutex, OnceLock};
use std::thread::spawn;

use futures::executor::block_on;
use many_cpus::SystemHardware;
use many_cpus::fake::HardwareBuilder;
use new_zealand::nz;
use ohno::ErrorExt;
use testing::with_watchdog;
use vicinal::Pool;
use vicinal::worker_policy::{
    BindContext, BindError, BoundPolicy, WorkerGroupId, WorkerGroups, WorkerPolicy,
};

testing::init_tracing!();

#[derive(Debug)]
/// Lazily creates one route on first public selection to test custom policy startup.
struct LazySingleGroupPolicy {
    group: OnceLock<WorkerGroupId>,
}

/// Reports an ordinary policy-specific binding failure.
#[derive(Debug)]
struct FailingBindingPolicy {
    initializer: Mutex<Receiver<()>>,
    bound: Sender<()>,
    exited: Sender<()>,
}

impl WorkerPolicy for FailingBindingPolicy {
    fn bind(self, context: BindContext<'_>) -> Result<impl BoundPolicy, BindError> {
        let worker = context.groups().add().add_worker();
        let exited = self.exited.clone();
        _ = spawn(move || {
            worker.run();
            exited.send(()).unwrap();
        });
        self.initializer.lock().unwrap().recv().unwrap();
        self.bound.send(()).unwrap();

        Err::<Self, _>(BindError::new(IoError::other(
            "injected policy binding failure",
        )))
    }
}

impl BoundPolicy for FailingBindingPolicy {
    fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
        unreachable!("construction rejects the binding failure")
    }
}

impl WorkerPolicy for LazySingleGroupPolicy {
    fn bind(self, _context: BindContext<'_>) -> Result<impl BoundPolicy, BindError> {
        Ok(self)
    }
}

impl BoundPolicy for LazySingleGroupPolicy {
    fn select_group(&self, groups: &WorkerGroups) -> WorkerGroupId {
        *self.group.get_or_init(|| {
            let group = groups.add();
            _ = spawn({
                let worker = group.add_worker();
                move || worker.run()
            });
            group.id()
        })
    }
}

#[test]
fn custom_policy_lazily_creates_and_runs_worker() {
    with_watchdog(|| {
        let pool = Pool::builder()
            .hardware(SystemHardware::fake(HardwareBuilder::from_counts(
                nz!(1),
                nz!(1),
            )))
            .worker_policy(LazySingleGroupPolicy {
                group: OnceLock::new(),
            })
            .build()
            .unwrap();

        assert_eq!(block_on(pool.scheduler().spawn(|| 42)).unwrap(), 42);
    });
}

#[test]
fn binding_failure_is_returned_after_started_workers_leave() {
    with_watchdog(|| {
        let (initializer_entered, initializer_arrival) = channel();
        let (bound, policy_bound) = channel();
        let (release_initializer, initializer_release) = channel();
        let (exited, worker_exited) = channel();
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));

        let construction = spawn(move || {
            let initializer_release = Mutex::new(initializer_release);
            Pool::builder()
                .hardware(hardware)
                .worker_policy(FailingBindingPolicy {
                    initializer: Mutex::new(initializer_arrival),
                    bound,
                    exited,
                })
                .thread_init(move || {
                    initializer_entered.send(()).unwrap();
                    initializer_release.lock().unwrap().recv().unwrap();
                })
                .build()
                .map(drop)
        });

        policy_bound.recv().unwrap();
        assert!(!construction.is_finished());
        release_initializer.send(()).unwrap();
        let error = construction.join().unwrap().unwrap_err();
        worker_exited.recv().unwrap();
        let cause = error.source().unwrap().downcast_ref::<BindError>().unwrap();
        assert!(cause.find_source::<IoError>().is_some());
        _ = error.to_string();
        _ = cause.to_string();
    });
}

#[test]
fn foreign_eager_hardware_returns_build_error() {
    let pool_hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
    let eager_hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));

    let result = Pool::builder()
        .hardware(pool_hardware)
        .eager(eager_hardware.all_processors())
        .build();

    _ = result.unwrap_err();
}

::testing::set_allocator!();
