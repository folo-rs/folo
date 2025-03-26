use std::time::Instant;

use many_cpus::ProcessorSet;

fn main() {
    // We spawn N threads, where N is the number of processors.
    // However, we do not pin them to any specific processor.
    // This means that the OS can schedule them however it likes.

    let processor_set = ProcessorSet::all();

    let mut all_threads = Vec::with_capacity(processor_set.len());

    for _ in 0..processor_set.len() {
        let thread = std::thread::spawn(move || {
            let start = Instant::now();

            let mut x: usize = 0;

            loop {
                for _ in 0..100_000 {
                    x += 1;
                }

                // Every thread spins the CPU for 10 seconds.
                if start.elapsed().as_secs() > 10 {
                    println!("Thread finished after {x} iterations");
                    break;
                }
            }
        });

        all_threads.push(thread);
    }

    println!("Spawned {} threads", all_threads.len());

    for thread in all_threads {
        thread.join().unwrap();
    }

    println!("All threads have finished.");
}
