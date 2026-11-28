#![expect(
    missing_docs,
    reason = "This is a test file, documentation is not required."
)]

use std::error::Error;
use std::io::Error as IoError;
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, Sender, channel};
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

/// Lazily creates one route on first public selection to test custom policy startup.
#[derive(Debug)]
struct LazySingleGroupPolicy {
    group: OnceLock<WorkerGroupId>,
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

/// Reports an ordinary policy-specific binding failure after the worker enters initialization.
#[derive(Debug)]
struct FailingBindingPolicy {
    initializer: Receiver<()>,
    exited: Sender<()>,
}

impl WorkerPolicy for FailingBindingPolicy {
    fn bind(self, context: BindContext<'_>) -> Result<impl BoundPolicy, BindError> {
        let worker = context.groups().add().add_worker();
        _ = spawn(move || {
            worker.run();
            self.exited.send(()).unwrap();
        });
        self.initializer.recv().unwrap();

        Err::<NeverBound, _>(BindError::new(IoError::other(
            "injected policy binding failure",
        )))
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
fn binding_failure_retains_policy_error_and_worker_exits() {
    with_watchdog(|| {
        let (initializer_entered, initializer_arrival) = channel();
        let (exited, worker_exited) = channel();
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));

        let error = Pool::builder()
            .hardware(hardware)
            .worker_policy(FailingBindingPolicy {
                initializer: initializer_arrival,
                exited,
            })
            .thread_init(move || initializer_entered.send(()).unwrap())
            .build()
            .unwrap_err();
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
