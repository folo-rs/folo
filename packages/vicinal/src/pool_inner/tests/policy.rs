use std::num::NonZero;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::spawn;

use futures::executor::block_on;
use many_cpus::fake::HardwareBuilder;
use many_cpus::{ProcessorSet, SystemHardware};
use new_zealand::nz;
use testing::with_watchdog;

use crate::policies::{WorkersPerMemoryRegion, WorkersPerSystem};
use crate::{
    GroupId, Pool, PrepareContext, Scheduler, WorkerGroup, WorkerGroupId, WorkerGroups,
    WorkerPolicy,
};

#[derive(Debug)]
/// Prepares one stable ID-only route to verify ownership without cloning on selection.
struct SingleGroupPolicy {
    group: OnceLock<WorkerGroupId>,
}

impl WorkerPolicy for SingleGroupPolicy {
    fn prepare(&mut self, context: PrepareContext<'_>) {
        let group = context.groups().add();
        let id = group.id();
        self.group.set(id).unwrap();

        let affinity = context.hardware().processors();
        let worker = group.add_worker();
        _ = spawn(move || {
            affinity.pin_current_thread_to();
            worker.run();
        });
    }

    fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
        *self.group.get().unwrap()
    }
}

/// Models a startup tracing subscriber that submits through the route still being created.
struct ReentrantStartupPolicy {
    group: OnceLock<WorkerGroup>,
    affinity: OnceLock<ProcessorSet>,
    started: OnceLock<WorkerGroupId>,
    scheduler: Arc<Mutex<Option<Scheduler>>>,
}

impl WorkerPolicy for ReentrantStartupPolicy {
    fn prepare(&mut self, context: PrepareContext<'_>) {
        let group = context.groups().add();
        assert!(
            group
                .state()
                .before_startup_trace
                .set(Box::new({
                    let scheduler = Arc::clone(&self.scheduler);
                    move || {
                        scheduler
                            .lock()
                            .unwrap()
                            .clone()
                            .expect("the scheduler is installed before lazy startup")
                            .spawn_and_forget(|| {});
                    }
                }))
                .is_ok()
        );
        self.group.set(group).unwrap();
        self.affinity.set(context.hardware().processors()).unwrap();
    }

    fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
        *self.started.get_or_init(|| {
            let group = self.group.get().unwrap();
            group.start_workers(
                GroupId::Processor(0),
                self.affinity.get().unwrap(),
                nz!(1_u32),
            );
            group.id()
        })
    }
}

#[test]
fn custom_policy_selects_owned_group_without_cloning() {
    with_watchdog(|| {
        let pool = Pool::builder()
            .hardware(SystemHardware::fake(HardwareBuilder::from_counts(
                nz!(1),
                nz!(1),
            )))
            .worker_policy(SingleGroupPolicy {
                group: OnceLock::new(),
            })
            .build();

        assert_eq!(block_on(pool.scheduler().spawn(|| 42)).unwrap(), 42);
    });
}

#[test]
fn startup_trace_can_submit_through_participating_route() {
    with_watchdog(|| {
        let scheduler = Arc::new(Mutex::new(None));
        let pool = Pool::builder()
            .hardware(SystemHardware::fake(HardwareBuilder::from_counts(
                nz!(1),
                nz!(1),
            )))
            .worker_policy(ReentrantStartupPolicy {
                group: OnceLock::new(),
                affinity: OnceLock::new(),
                started: OnceLock::new(),
                scheduler: Arc::clone(&scheduler),
            })
            .build();
        let pool_scheduler = pool.scheduler();
        *scheduler.lock().unwrap() = Some(pool_scheduler.clone());

        assert_eq!(block_on(pool_scheduler.spawn(|| 42)).unwrap(), 42);
        *scheduler.lock().unwrap() = None;
    });
}

#[test]
fn last_built_in_policy_setter_wins() {
    with_watchdog(|| {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(2), nz!(1)));
        let pool = Pool::builder()
            .hardware(hardware)
            .worker_policy(WorkersPerSystem::new(NonZero::new(3).unwrap()))
            .worker_policy(WorkersPerMemoryRegion::new(nz!(1)))
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
    assert_eq!(cloned.worker_count(), 0);
}
