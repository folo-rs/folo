use futures::executor::block_on;
use many_cpus::SystemHardware;
use many_cpus::fake::HardwareBuilder;
use new_zealand::nz;
use testing::with_watchdog;

use crate::policies::{WorkersPerMemoryRegion, WorkersPerProcessor, WorkersPerSystem};
use crate::{Pool, WorkerPolicy};

/// Verifies that every public facade reaches the shared worker implementation.
#[test]
fn built_in_policy_facades_execute_tasks() {
    assert_executes(WorkersPerProcessor::new(nz!(1_u32)));
    assert_executes(WorkersPerMemoryRegion::new(nz!(1_u32)));
    assert_executes(WorkersPerSystem::new(nz!(1_u32)));
}

/// Runs one representative task through a supplied built-in policy.
fn assert_executes(policy: impl WorkerPolicy) {
    with_watchdog(move || {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
        let pool = Pool::builder()
            .hardware(hardware)
            .worker_policy(policy)
            .build();

        assert_eq!(block_on(pool.scheduler().spawn(|| 42)).unwrap(), 42);
    });
}
