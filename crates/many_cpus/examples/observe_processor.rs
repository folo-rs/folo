use std::{thread, time::Duration};

fn main() {
    loop {
        let current_processor_id = many_cpus::current_processor_id();
        let current_memory_region_id = many_cpus::current_memory_region_id();

        println!(
            "Thread executing on processor {current_processor_id} in memory region {current_memory_region_id}"
        );

        thread::sleep(Duration::from_secs(1));
    }
}
