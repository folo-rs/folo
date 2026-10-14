//! Example from the README.

use new_zealand::nz;
use ohno::AppError;
use vicinal::{Pool, TaskError, WorkerPolicy};

#[tokio::main]
async fn main() -> Result<(), AppError> {
    processor_local().await?;
    memory_region_local().await?;
    Ok(())
}

async fn processor_local() -> Result<(), AppError> {
    let pool = Pool::new();
    let scheduler = pool.scheduler();

    let task1 = scheduler.spawn(|| 42);
    let _task2 = scheduler.spawn(|| println!("doing some stuff"));

    assert_eq!(task1.await.map_err(TaskError::into_diagnostic)?, 42);

    let scheduler2 = scheduler.clone();
    let task3 = scheduler.spawn(move || scheduler2.spawn(|| 55));

    let task3 = task3.await.map_err(TaskError::into_diagnostic)?;
    assert_eq!(task3.await.map_err(TaskError::into_diagnostic)?, 55);
    Ok(())
}

async fn memory_region_local() -> Result<(), AppError> {
    // Workers share each region's cumulative set of submitting processors.
    // Illustrative concurrency; choose the count for your blocking workload.
    let pool = Pool::builder()
        .worker_policy(WorkerPolicy::per_memory_region(nz!(2_u32)))
        .build();

    assert_eq!(
        pool.scheduler()
            .spawn(|| 42)
            .await
            .map_err(TaskError::into_diagnostic)?,
        42
    );
    Ok(())
}

::testing::set_allocator!();
