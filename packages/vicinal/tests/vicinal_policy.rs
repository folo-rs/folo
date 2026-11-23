#![expect(
    missing_docs,
    reason = "This is a test file, documentation is not required."
)]

use std::sync::OnceLock;
use std::thread::spawn;

use futures::executor::block_on;
use many_cpus::SystemHardware;
use many_cpus::fake::HardwareBuilder;
use new_zealand::nz;
use testing::with_watchdog;
use vicinal::{Pool, WorkerGroupId, WorkerGroups, WorkerPolicy};

testing::init_tracing!();

#[derive(Debug)]
/// Lazily creates one route on first public selection to test custom policy startup.
struct LazySingleGroupPolicy {
    group: OnceLock<WorkerGroupId>,
}

impl WorkerPolicy for LazySingleGroupPolicy {
    fn select_group(&self, groups: &WorkerGroups) -> WorkerGroupId {
        *self.group.get_or_init(|| {
            let group = groups.add();
            _ = spawn({
                let group = group.clone();
                move || group.execute_worker()
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
            .build();

        assert_eq!(block_on(pool.scheduler().spawn(|| 42)).unwrap(), 42);
    });
}

::testing::set_allocator!();
