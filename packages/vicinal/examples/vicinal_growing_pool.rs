//! Demonstrates a custom policy that grows without a fixed worker limit.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::channel;
use std::thread::spawn;

use futures::executor::block_on;
use vicinal::{Pool, WorkerGroup, WorkerGroupId, WorkerGroups, WorkerPolicy, WorkerPolicyContext};

fn main() {
    let worker_count = Arc::new(AtomicUsize::new(0));
    let pool = Pool::builder()
        .worker_policy(GrowingPolicy::new(Arc::clone(&worker_count)))
        .build();
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
    // `second_arrival` completes after the second worker increments the counter above.
    assert_eq!(worker_count.load(Ordering::Acquire), 2);

    release_first
        .send(())
        .expect("the initial worker is waiting for release");
    assert_eq!(block_on(first).expect("the first task succeeds"), 1);
    assert_eq!(block_on(second).expect("the second task succeeds"), 2);
    assert_eq!(block_on(third).expect("the third task succeeds"), 3);
}

/// Adds one worker for each submission that observes queued work in the sole group.
///
/// Policy selection precedes queue admission, so the incoming task itself is not visible in
/// `queue_len()`. This policy starts one worker eagerly and grows when a later submission
/// observes backlog. It deliberately never removes workers or imposes a maximum.
struct GrowingPolicy {
    group: Option<WorkerGroup>,
    worker_count: Arc<AtomicUsize>,
}

impl GrowingPolicy {
    fn new(worker_count: Arc<AtomicUsize>) -> Self {
        Self {
            group: None,
            worker_count,
        }
    }
}

impl WorkerPolicy for GrowingPolicy {
    fn prepare(&mut self, context: WorkerPolicyContext<'_>) {
        let group = context.groups().add();
        spawn_worker(&group, &self.worker_count);
        self.group = Some(group);
    }

    fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
        let group = self.group.as_ref().expect("build prepares the policy");
        if group.queue_len() != 0 {
            spawn_worker(group, &self.worker_count);
        }

        group.id()
    }
}

/// Transfers one group handle into a policy-owned OS thread and records its startup.
fn spawn_worker(group: &WorkerGroup, worker_count: &Arc<AtomicUsize>) {
    _ = spawn({
        let worker = group.add_worker();
        let worker_count = Arc::clone(worker_count);
        move || {
            // The task handshakes below establish that this increment happened before the
            // assertion. Release/Acquire only describes the counter update; it is not a
            // substitute for waiting for the worker to enter task execution.
            worker_count.fetch_add(1, Ordering::Release);
            worker.run();
        }
    });
}

::testing::set_allocator!();
