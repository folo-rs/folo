//! Demonstrates workers that share a caller's memory region without requiring its processor.

use futures::executor::block_on;
use many_cpus::SystemHardware;
use new_zealand::nz;
use vicinal::{Pool, WorkerPolicy};

fn main() {
    let hardware = SystemHardware::current();
    // Keep the caller's region stable between observing it and submitting the task.
    let caller = hardware
        .all_processors()
        .take(nz!(1))
        .expect("the system has a processor");
    caller.pin_current_thread_to();
    let caller_region = hardware.current_memory_region_id();
    let pool = Pool::builder()
        .worker_policy(WorkerPolicy::fixed_per_memory_region(nz!(2_u32)))
        .build();
    let region = block_on(
        pool.scheduler()
            .spawn(|| SystemHardware::current().current_memory_region_id()),
    );
    assert_eq!(region, caller_region);
    println!("Task executed in memory region {region}");
}

::testing::set_allocator!();
