//! Example demonstrating priority processing of urgent tasks.
//!
//! This example spawns 200 regular tasks followed by 200 urgent tasks.
//! Despite being spawned later, the urgent tasks are processed with higher priority
//! and complete before the remaining regular tasks.

use std::thread;
use std::time::Duration;

use ohno::AppError;
use vicinal::{Pool, TaskError};

#[tokio::main]
async fn main() -> Result<(), AppError> {
    let pool = Pool::new();
    let scheduler = pool.scheduler();

    let mut handles = Vec::new();

    println!("Spawning 200 regular tasks...");
    for i in 1..=200 {
        let handle = scheduler.spawn(move || {
            thread::sleep(Duration::from_millis(10));
            println!("Regular task {i} finished");
        });
        handles.push(handle);
    }

    println!("Spawning 200 urgent tasks...");
    for i in 1..=200 {
        let handle = scheduler.spawn_urgent(move || {
            thread::sleep(Duration::from_millis(10));
            println!("Urgent task {i} finished");
        });
        handles.push(handle);
    }

    println!("Waiting for tasks to complete...");
    for result in futures::future::join_all(handles).await {
        result.map_err(TaskError::into_diagnostic)?;
    }
    Ok(())
}

::testing::set_allocator!();
