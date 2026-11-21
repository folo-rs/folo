use std::num::NonZero;
use std::sync::OnceLock;
use std::thread;

use futures::executor::block_on;
use many_cpus::SystemHardware;
use many_cpus::fake::HardwareBuilder;
use new_zealand::nz;
use testing::with_watchdog;

use crate::{
    DefaultWorkerPolicy, Pool, WorkerGroupId, WorkerGroups, WorkerPolicy, WorkerPolicyContext,
};

#[derive(Debug)]
struct SingleGroupPolicy {
    group: OnceLock<WorkerGroupId>,
}

impl WorkerPolicy for SingleGroupPolicy {
    fn prepare(&mut self, context: WorkerPolicyContext<'_>) {
        let group = context.groups().add();
        let id = group.id();
        self.group.set(id).unwrap();

        let affinity = SystemHardware::current().processors();
        thread::spawn(move || {
            affinity.pin_current_thread_to();
            group.execute_worker();
        });
    }

    fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
        *self.group.get().unwrap()
    }
}

#[test]
fn custom_policy_selects_owned_group_without_cloning() {
    with_watchdog(|| {
        let pool = Pool::builder()
            .worker_policy(SingleGroupPolicy {
                group: OnceLock::new(),
            })
            .build();

        assert_eq!(block_on(pool.scheduler().spawn(|| 42)).unwrap(), 42);
    });
}

#[test]
fn last_default_policy_setter_wins() {
    with_watchdog(|| {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(2), nz!(1)));
        let pool = Pool::builder()
            .hardware(hardware)
            .worker_policy(DefaultWorkerPolicy::per_system(NonZero::new(3).unwrap()))
            .worker_policy(DefaultWorkerPolicy::per_memory_region(nz!(1)))
            .build();

        assert_eq!(block_on(pool.scheduler().spawn(|| 7)).unwrap(), 7);
        assert_eq!(pool.inner.groups.len(), 1);
        assert_eq!(pool.inner.runtime.active_workers(), 1);
    });
}

#[test]
fn group_and_id_are_copy_clone_safe() {
    let pool = Pool::builder()
        .hardware(SystemHardware::fake(HardwareBuilder::from_counts(
            nz!(1),
            nz!(1),
        )))
        .build();
    let group = pool.inner.groups.add();
    let id = group.id();
    let cloned = group.clone();

    assert_eq!(id, group.id());
    assert_eq!(cloned.len(), 0);
}
