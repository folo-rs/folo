use std::sync::OnceLock;
use std::{panic, thread};

use many_cpus::SystemHardware;
use many_cpus::fake::HardwareBuilder;
use new_zealand::nz;
use testing::{assert_panics, with_watchdog};

use crate::{Pool, WorkerGroupId, WorkerGroups, WorkerPolicy, WorkerPolicyContext};

struct StartupFailurePolicy {
    group: OnceLock<WorkerGroupId>,
}

impl WorkerPolicy for StartupFailurePolicy {
    fn prepare(&mut self, context: WorkerPolicyContext<'_>) {
        let group = context.groups().add();
        self.group.set(group.id()).unwrap();
        thread::spawn(move || group.execute_worker());
        panic!("injected eager policy preparation failure");
    }

    fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
        *self.group.get().unwrap()
    }
}

#[test]
fn policy_preparation_failure_stops_started_workers() {
    with_watchdog(|| {
        assert_panics(|| {
            _ = Pool::builder()
                .worker_policy(StartupFailurePolicy {
                    group: OnceLock::new(),
                })
                .build();
        });
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
