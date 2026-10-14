//! Demonstrates regional workers following a stable quota-aware set of submitting processors.

use futures::executor::block_on;
use many_cpus::SystemHardware;
use new_zealand::nz;
use ohno::AppError;
use vicinal::{Pool, TaskError, WorkerPolicy};

fn main() -> Result<(), AppError> {
    let hardware = SystemHardware::current();
    // Select the orchestrator's stable CPU set once. Worker placement follows this set;
    // fractional quotas and unrelated process work may still cause throttling.
    let submitters = hardware.processors();
    // Illustrative concurrency; choose the count for your blocking workload.
    let pool = Pool::builder()
        .worker_policy(WorkerPolicy::per_memory_region(nz!(2_u32)))
        .build();
    let scheduler = pool.scheduler();
    let callers = submitters.spawn_threads(move |processor| -> Result<(), AppError> {
        let caller_region = processor.memory_region_id();
        let region =
            block_on(scheduler.spawn(|| SystemHardware::current().current_memory_region_id()))
                .map_err(TaskError::into_diagnostic)?;
        assert_eq!(region, caller_region);
        println!("Task executed in memory region {region}");
        Ok(())
    });
    for caller in callers {
        caller.join().expect("submitting thread must succeed")?;
    }
    Ok(())
}

::testing::set_allocator!();
