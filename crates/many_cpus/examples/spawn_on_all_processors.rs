use many_cpus::ProcessorSet;

fn main() {
    let all_threads = ProcessorSet::all().spawn_threads(|processor| {
        println!("Spawned thread on processor {}", processor.id());

        // In a real service, you would start some work handler here, e.g. to read
        // and process messages from a channel or to spawn a web handler.
    });

    for thread in all_threads {
        thread.join().unwrap();
    }

    println!("All threads have finished.");
}
