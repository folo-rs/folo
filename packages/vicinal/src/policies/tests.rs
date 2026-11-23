use futures::executor::block_on;
use many_cpus::SystemHardware;
use many_cpus::fake::HardwareBuilder;
use new_zealand::nz;
use testing::with_watchdog;

use crate::policies::{WorkersPerMemoryRegion, WorkersPerProcessor, WorkersPerSystem};
use crate::{Pool, WorkerPolicy, WorkerPolicyContext};

/// Verifies that every public facade reaches the shared worker implementation.
#[test]
fn built_in_policy_facades_execute_tasks() {
    assert_executes(WorkersPerProcessor::new(nz!(1_u32)));
    assert_executes(WorkersPerMemoryRegion::new(nz!(1_u32)));
    assert_executes(WorkersPerSystem::new(nz!(1_u32)));
}

/// Exercises each new diagnostic type through its valid empty and prepared states.
#[test]
fn public_policy_values_format() {
    let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
    let processor = WorkersPerProcessor::new(nz!(1));
    let region = WorkersPerMemoryRegion::new(nz!(2));
    let system = WorkersPerSystem::new(nz!(3));
    let pool = Pool::builder()
        .hardware(hardware.clone())
        .worker_policy(processor.clone())
        .build();
    let groups = &pool.inner.groups;
    let group = groups.add();
    let id = group.id();
    let eager = hardware.all_processors();
    let context = WorkerPolicyContext::new(&hardware, None, groups, "formatting");
    let eager_context = WorkerPolicyContext::new(&hardware, Some(&eager), groups, "formatting");

    _ = format!("{processor:?}");
    _ = format!("{region:?}");
    _ = format!("{system:?}");
    _ = format!("{groups:?}");
    _ = format!("{group:?}");
    _ = format!("{id:?}");
    _ = format!("{context:?}");
    _ = format!("{eager_context:?}");
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
