//! Example demonstrating the processor-locality aspect of the vicinal worker pool.
//!
//! This example spawns a task every 100ms and prints the processor it is executing on.
//! As the main thread is not pinned, it may migrate between processors, and the tasks
//! should follow it.

use std::thread;
use std::time::Duration;

use futures::executor::block_on;
use many_cpus::SystemHardware;
use vicinal::Pool;

fn main() {
    // Exit early if running in a testing environment
    if std::env::var("IS_TESTING").is_ok() {
        println!("Running in testing mode - exiting immediately to prevent infinite loop");
        return;
    }

    println!("=== Processor Locality Demo ===");
    println!("Spawning a task every 100ms. Press Ctrl+C to stop.");
    println!();

    let pool = Pool::new();
    let scheduler = pool.scheduler();

    loop {
        let handle = scheduler.spawn(|| {
            let processor_id = SystemHardware::current().current_processor_id();
            println!("Task executed on processor {processor_id}");
        });

        block_on(handle);
        thread::sleep(Duration::from_millis(100));
    }
}
