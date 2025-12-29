//! Example demonstrating spawn_and_forget() and spawn_urgent_and_forget().

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use vicinal::Pool;

fn main() {
    let pool = Pool::new();
    let scheduler = pool.scheduler();

    // Counter to track completed tasks.
    let counter = Arc::new(AtomicUsize::new(0));

    println!("Spawning 10 regular tasks with spawn_and_forget()...");
    for i in 0..10 {
        let counter = Arc::clone(&counter);
        scheduler.spawn_and_forget(move || {
            println!("Regular task {} executing", i);
            counter.fetch_add(1, Ordering::Relaxed);
        });
    }

    println!("Spawning 5 urgent tasks with spawn_urgent_and_forget()...");
    for i in 0..5 {
        let counter = Arc::clone(&counter);
        scheduler.spawn_urgent_and_forget(move || {
            println!("Urgent task {} executing", i);
            counter.fetch_add(1, Ordering::Relaxed);
        });
    }

    // Give tasks time to complete (since we don't have join handles).
    thread::sleep(Duration::from_millis(100));

    let completed = counter.load(Ordering::Relaxed);
    println!("Completed {} tasks", completed);

    assert_eq!(
        completed, 15,
        "Expected all 15 tasks to complete, but only {} completed",
        completed
    );

    println!("All tasks completed successfully!");
}
