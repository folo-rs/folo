//! Spawns one thread on each processor in the default processor set.

use many_cpus::SystemHardware;

fn main() {
    let threads = SystemHardware::current()
        .processors()
        .spawn_threads(|processor| {
            println!("Spawned thread on processor {}", processor.id());

            // In a real service, you would start some work handler here, e.g. to read
            // and process messages from a channel or to spawn a web handler.
        });

    for thread in threads {
        thread.join().unwrap();
    }

    println!("All threads have finished.");
}
