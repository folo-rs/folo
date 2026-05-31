//! Selects a pair of processors and spawns a thread on each of them.
//! This demonstrates arbitrary processor selection logic.

use many_cpus::SystemHardware;
use new_zealand::nz;

fn main() {
    let hw = SystemHardware::current();

    let selected_processors = hw
        .processors()
        .to_builder()
        .same_memory_region()
        .performance_processors_only()
        .take(nz!(2))
        // If we do not have what we want, we fall back to the default set.
        .unwrap_or_else(|| hw.processors());

    let threads = selected_processors.spawn_threads(|processor| {
        println!("Spawned thread on processor {}", processor.id());

        // In a real service, you would start some work handler here, e.g. to read
        // and process messages from a channel or to spawn a web handler.
    });

    for thread in threads {
        thread.join().unwrap();
    }

    println!("All threads have finished.");
}
