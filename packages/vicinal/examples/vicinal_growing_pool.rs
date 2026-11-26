//! Demonstrates a custom policy that grows without a fixed worker limit.

use std::sync::mpsc::channel;
use std::thread::spawn;

use futures::executor::block_on;
use vicinal::{
    Pool, PrepareContext, PrepareError, WorkerGroup, WorkerGroupId, WorkerGroups, WorkerPolicy,
};

fn main() -> Result<(), PrepareError> {
    let pool = Pool::builder()
        .worker_policy(GrowingPolicy::new())
        .build()?;
    let scheduler = pool.scheduler();

    let (first_entered, first_arrival) = channel();
    let (release_first, wait_for_release) = channel();
    let first = scheduler.spawn(move || {
        first_entered
            .send(())
            .expect("the example retains the first arrival receiver");
        wait_for_release
            .recv()
            .expect("the example releases the first worker");
        1
    });
    first_arrival
        .recv()
        .expect("the first worker enters the task");

    // The initial worker is occupied, so this task remains queued.
    let (second_entered, second_arrival) = channel();
    let second = scheduler.spawn(move || {
        second_entered
            .send(())
            .expect("the example retains the second arrival receiver");
        2
    });

    // Selecting a group for this task observes the queued second task and adds a worker.
    let third = scheduler.spawn(|| 3);
    second_arrival
        .recv()
        .expect("the additional worker enters the queued task");

    release_first
        .send(())
        .expect("the initial worker is waiting for release");
    assert_eq!(block_on(first).expect("the first task succeeds"), 1);
    assert_eq!(block_on(second).expect("the second task succeeds"), 2);
    assert_eq!(block_on(third).expect("the third task succeeds"), 3);
    Ok(())
}

/// Adds one worker for each submission that observes queued work in the sole group.
///
/// Policy selection precedes queue admission, so the incoming task itself is not visible in
/// `queue_len()`. This policy starts one worker eagerly and grows when a later submission
/// observes backlog. It deliberately never removes workers or imposes a maximum.
struct GrowingPolicy {
    group: Option<WorkerGroup>,
}

impl GrowingPolicy {
    fn new() -> Self {
        Self { group: None }
    }
}

impl WorkerPolicy for GrowingPolicy {
    fn prepare(&mut self, context: PrepareContext<'_>) -> Result<(), PrepareError> {
        let group = context.groups().add();
        spawn_worker(&group);
        self.group = Some(group);
        Ok(())
    }

    fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
        let group = self.group.as_ref().expect("build prepares the policy");
        if group.queue_len() != 0 {
            spawn_worker(group);
        }

        group.id()
    }
}

/// Borrows the group to reserve a worker before moving only that worker into an OS thread.
fn spawn_worker(group: &WorkerGroup) {
    _ = spawn({
        let worker = group.add_worker();
        move || worker.run()
    });
}

::testing::set_allocator!();
