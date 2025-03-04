use std::{thread, time::Duration};

use many_cpus::ProcessorSet;

fn main() {
    // The set of processors used here can be adjusted via platform commands.
    //
    // For example, to select only processors 0 and 1:
    // Linux: taskset 0x3 target/debug/examples/spawn_on_inherited_processors
    // Windows: start /affinity 0x3 target/debug/examples/spawn_on_inherited_processors.exe

    let inherited_processors = ProcessorSet::builder()
        .where_available_for_current_thread()
        .take_all()
        .expect("found no processors usable by the current thread - impossible because the thread is currently running on one");

    println!(
        "The current thread has inherited an affinity to {} processors.",
        inherited_processors.len()
    );

    let all_threads = inherited_processors.spawn_threads(|processor| {
        println!("Spawned thread on processor {}", processor.id());

        // In a real service, you would start some work handler here, e.g. to read
        // and process messages from a channel or to spawn a web handler.
    });

    for thread in all_threads {
        thread.join().unwrap();
    }

    println!("All threads have finished. Exiting in 10 seconds.");

    // Give some time to exit, as on Windows using "start" will create a new window that would
    // otherwise disappear instantly, making it hard to see what happened.
    thread::sleep(Duration::from_secs(10));
}
