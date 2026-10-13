//! Example from the README.

use new_zealand::nz;
use vicinal::{Pool, WorkerPolicy};

#[tokio::main]
async fn main() {
    processor_local().await;
    memory_region_local().await;
}

async fn processor_local() {
    let pool = Pool::new();
    let scheduler = pool.scheduler();

    let task1 = scheduler.spawn(|| 42);
    let _task2 = scheduler.spawn(|| println!("doing some stuff"));

    assert_eq!(task1.await, 42);

    let scheduler2 = scheduler.clone();
    let task3 = scheduler.spawn(move || scheduler2.spawn(|| 55));

    assert_eq!(task3.await.await, 55);
}

async fn memory_region_local() {
    // Workers share each region's cumulative set of submitting processors.
    // Illustrative concurrency; choose the count for your blocking workload.
    let pool = Pool::builder()
        .worker_policy(WorkerPolicy::fixed_per_memory_region(nz!(2_u32)))
        .build();

    assert_eq!(pool.scheduler().spawn(|| 42).await, 42);
}

::testing::set_allocator!();
