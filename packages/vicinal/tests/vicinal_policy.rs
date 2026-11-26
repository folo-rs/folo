#![expect(
    missing_docs,
    reason = "This is a test file, documentation is not required."
)]

use std::io;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Mutex, OnceLock};
use std::thread::spawn;

use futures::executor::block_on;
use many_cpus::SystemHardware;
use many_cpus::fake::HardwareBuilder;
use new_zealand::nz;
use ohno::ErrorExt;
use testing::with_watchdog;
use vicinal::{Pool, PrepareContext, PrepareError, WorkerGroupId, WorkerGroups, WorkerPolicy};

testing::init_tracing!();

#[derive(Debug)]
/// Lazily creates one route on first public selection to test custom policy startup.
struct LazySingleGroupPolicy {
    group: OnceLock<WorkerGroupId>,
}

/// Reports an ordinary policy-specific preparation failure.
struct FailingPreparationPolicy {
    initializer: Mutex<Receiver<()>>,
    prepared: Sender<()>,
    exited: Sender<()>,
}

impl WorkerPolicy for FailingPreparationPolicy {
    fn prepare(&mut self, context: PrepareContext<'_>) -> Result<(), PrepareError> {
        let worker = context.groups().add().add_worker();
        let exited = self.exited.clone();
        _ = spawn(move || {
            worker.run();
            exited.send(()).unwrap();
        });
        self.initializer.lock().unwrap().recv().unwrap();
        self.prepared.send(()).unwrap();

        Err(PrepareError::new(io::Error::other(
            "injected policy preparation failure",
        )))
    }

    fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
        unreachable!("construction rejects the preparation failure")
    }
}

impl WorkerPolicy for LazySingleGroupPolicy {
    fn prepare(&mut self, _context: PrepareContext<'_>) -> Result<(), PrepareError> {
        Ok(())
    }

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
fn preparation_failure_is_returned_after_started_workers_leave() {
    with_watchdog(|| {
        let (initializer_entered, initializer_arrival) = channel();
        let (prepared, policy_prepared) = channel();
        let (release_initializer, initializer_release) = channel();
        let (exited, worker_exited) = channel();
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));

        let construction = spawn(move || {
            let initializer_release = Mutex::new(initializer_release);
            Pool::builder()
                .hardware(hardware)
                .worker_policy(FailingPreparationPolicy {
                    initializer: Mutex::new(initializer_arrival),
                    prepared,
                    exited,
                })
                .thread_init(move || {
                    initializer_entered.send(()).unwrap();
                    initializer_release.lock().unwrap().recv().unwrap();
                })
                .build()
                .map(drop)
        });

        policy_prepared.recv().unwrap();
        assert!(!construction.is_finished());
        release_initializer.send(()).unwrap();
        let error = construction.join().unwrap().unwrap_err();
        worker_exited.recv().unwrap();
        assert!(error.find_source::<io::Error>().is_some());
    });
}

::testing::set_allocator!();
