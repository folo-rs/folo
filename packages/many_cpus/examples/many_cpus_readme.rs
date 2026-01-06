//! Example that demonstrates the exact usage shown in the README.md file.
//!
//! This shows how to use `SystemHardware` to spawn threads on many processors.

use many_cpus::SystemHardware;

fn main() {
    println!("=== Many CPUs README Example ===");

    let threads = SystemHardware::current()
        .processors()
        .spawn_threads(|processor| {
            println!("Spawned thread on processor {}", processor.id());

            // In a real service, you would start some work handler here, e.g. to read
            // and process messages from a channel or to spawn a web handler.
        });

    // Wait for all threads to complete
    for thread in threads {
        thread.join().unwrap();
    }

    println!("README example completed successfully!");
}
