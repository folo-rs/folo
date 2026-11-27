//! Demonstrates a custom policy that grows without a fixed worker limit.

use std::io;
use std::sync::mpsc::channel;
use std::thread::Builder;

use futures::executor::block_on;
use ohno::AppError;
use vicinal::Pool;
use vicinal::worker_policy::{
    BindContext, BindError, BoundPolicy, Worker, WorkerGroup, WorkerGroupId, WorkerGroups,
    WorkerPolicy,
};

fn main() -> Result<(), AppError> {
    let pool = Pool::builder().worker_policy(GrowingPolicy).build()?;
    drain_backlog(&pool);
    Ok(())
}

/// Exercises growth with an occupied initial worker and an existing queued task.
fn drain_backlog(pool: &Pool) {
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
    let second = scheduler.spawn(|| 2);

    // Selection sees the queued second task and tries to add a worker.
    let third = scheduler.spawn(|| 3);

    // Do not wait for another worker: if growth failed, the initial worker drains the queue.
    release_first
        .send(())
        .expect("the initial worker is waiting for release");
    assert_eq!(block_on(first).expect("the first task succeeds"), 1);
    assert_eq!(block_on(second).expect("the second task succeeds"), 2);
    assert_eq!(block_on(third).expect("the third task succeeds"), 3);
}

/// Starts one worker during binding and lets the bound policy grow the group.
#[derive(Debug)]
struct GrowingPolicy;

impl WorkerPolicy for GrowingPolicy {
    fn bind(self, context: BindContext<'_>) -> Result<impl BoundPolicy, BindError> {
        BoundGrowingPolicy::bind(context, spawn_worker)
    }
}

/// Attempts to add a worker for each submission that observes queued work in the sole group.
///
/// Policy selection precedes queue admission, so the incoming task itself is not visible in
/// `queue_len()`. This policy starts one worker during binding and tries to grow when a submission
/// observes backlog. A later task can observe the earlier task in the queue even when a worker
/// is idle. That brief visibility keeps this example simple; a production policy needs a
/// smarter way to distinguish transient admission from sustained backlog. This policy
/// deliberately never removes workers or imposes a maximum.
#[derive(Debug)]
struct BoundGrowingPolicy {
    group: WorkerGroup,
    // Separates thread creation from reservation so the example can exercise refusal on demand.
    spawn_worker: fn(Worker) -> io::Result<()>,
}

impl BoundGrowingPolicy {
    /// Requires initial capacity before returning a routable policy.
    fn bind(
        context: BindContext<'_>,
        spawn_worker: fn(Worker) -> io::Result<()>,
    ) -> Result<Self, BindError> {
        let group = context.groups().add();
        spawn_worker(group.add_worker()).map_err(BindError::new)?;
        Ok(Self {
            group,
            spawn_worker,
        })
    }
}

impl BoundPolicy for BoundGrowingPolicy {
    fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
        if self.group.queue_len() != 0 {
            // Growth is best-effort: refusal drops the unstarted worker, preserving the route.
            _ = (self.spawn_worker)(self.group.add_worker());
        }

        self.group.id()
    }
}

/// Starts a detached OS thread to run an already reserved worker.
fn spawn_worker(worker: Worker) -> io::Result<()> {
    Builder::new().spawn(move || worker.run()).map(drop)
}

::testing::set_allocator!();

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use many_cpus::SystemHardware;
    use many_cpus::fake::HardwareBuilder;
    use new_zealand::nz;
    use ohno::ErrorExt;

    use super::*;

    /// Simulates OS refusal in the chosen phase without exhausting thread capacity.
    #[derive(Debug)]
    enum RefuseWorker {
        Binding,
        Growth,
    }

    thread_local! {
        // The example binds and submits on the same thread.
        static GROWTH_REFUSED: Cell<bool> = const { Cell::new(false) };
    }

    impl WorkerPolicy for RefuseWorker {
        fn bind(self, context: BindContext<'_>) -> Result<impl BoundPolicy, BindError> {
            match self {
                Self::Binding => BoundGrowingPolicy::bind(context, refuse_worker),
                Self::Growth => {
                    GROWTH_REFUSED.set(false);
                    let mut policy = BoundGrowingPolicy::bind(context, spawn_worker)?;
                    policy.spawn_worker = |worker| {
                        GROWTH_REFUSED.set(true);
                        refuse_worker(worker)
                    };
                    Ok(policy)
                }
            }
        }
    }

    /// Drops the captured reservation as `Builder::spawn` does on refusal.
    fn refuse_worker(worker: Worker) -> io::Result<()> {
        drop(worker);
        Err(io::Error::other("simulated worker creation refusal"))
    }

    #[test]
    fn initial_refusal_is_a_binding_error_with_its_source() {
        let error = Pool::builder()
            .hardware(SystemHardware::fake(HardwareBuilder::from_counts(
                nz!(1),
                nz!(1),
            )))
            .worker_policy(RefuseWorker::Binding)
            .build()
            .unwrap_err();
        let binding = error.find_source::<BindError>().unwrap();
        let cause = binding.find_source::<io::Error>().unwrap();
        assert_eq!(cause.to_string(), "simulated worker creation refusal");
    }

    #[test]
    fn existing_worker_drains_backlog_after_growth_is_refused() {
        let pool = Pool::builder()
            .hardware(SystemHardware::fake(HardwareBuilder::from_counts(
                nz!(1),
                nz!(1),
            )))
            .worker_policy(RefuseWorker::Growth)
            .build()
            .unwrap();
        drain_backlog(&pool);
        assert!(GROWTH_REFUSED.get());
    }
}
