//! Selects a pair of processors and spawns a thread on each of them.
//! This demonstrates arbitrary processor selection logic.

use std::num::NonZero;

use many_cpus::ProcessorSet;

const PROCESSOR_COUNT: NonZero<usize> = NonZero::new(2).unwrap();

fn main() {
    let selected_processors = ProcessorSet::builder()
        .same_memory_region()
        .performance_processors_only()
        .take(PROCESSOR_COUNT)
        // If we do not have what we want, we fall back to default, just to make the example run.
        .unwrap_or_default();

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
