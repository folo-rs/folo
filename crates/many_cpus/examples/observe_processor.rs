use std::{thread, time::Duration};

use many_cpus::{HardwareInfo, HardwareTracker};

fn main() {
    let max_processors = HardwareInfo::max_processor_count();
    let max_memory_regions = HardwareInfo::max_memory_region_count();
    println!(
        "This system can support up to {max_processors} processors in {max_memory_regions} memory regions"
    );

    loop {
        let current_processor_id = HardwareTracker::current_processor_id();
        let current_memory_region_id = HardwareTracker::current_memory_region_id();

        println!(
            "Thread executing on processor {current_processor_id} in memory region {current_memory_region_id}"
        );

        thread::sleep(Duration::from_secs(1));
    }
}
