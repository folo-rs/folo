//! Starts one thread on every processor in the system, respecting resource quotas and allowing the
//! set of allowed processors to be inherited from the environment (based on user configuration).
//!
//! The set of processors used here can be adjusted via any suitable OS mechanisms.
//!
//! For example, to select only processors 0 and 1:
//! Linux: `taskset 0x3 target/debug/examples/spawn_on_inherited_processors`
//! Windows: `start /affinity 0x3 target/debug/examples/spawn_on_inherited_processors.exe`

use std::{thread, time::Duration};

use many_cpus::ProcessorSet;

fn main() {
    let inherited_processors = ProcessorSet::builder()
        // This causes soft limits on processor affinity to be respected.
        .where_available_for_current_thread()
        .take_all()
        .expect("found no processors usable by the current thread - impossible because the thread is currently running on one");

    println!(
        "After applying soft limits, we are allowed to use {} processors.",
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
